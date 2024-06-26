use crate::{
    core::{
        entities::{
            edges::{edge_ref::EdgeRef, edge_store::EdgeStore},
            nodes::node_store::NodeStore,
            LayerIds, VID,
        },
        storage::timeindex::{AsTime, TimeIndex, TimeIndexEntry, TimeIndexOps},
        utils::errors::GraphError,
        Prop,
    },
    db::{
        api::{
            mutation::internal::InheritMutationOps,
            properties::internal::InheritPropertiesOps,
            view::{internal::*, BoxedIter, IntoDynBoxed},
        },
        graph::graph::{graph_equal, InternalGraph},
    },
    prelude::*,
};
use itertools::{EitherOrBoth, Itertools};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    cmp::min,
    fmt::{Display, Formatter},
    iter,
    ops::{Deref, Range},
    path::Path,
    sync::Arc,
};

/// A graph view where an edge remains active from the time it is added until it is explicitly marked as deleted.
///
/// Note that the graph will give you access to all edges that were added at any point in time, even those that are marked as deleted.
/// The deletion only has an effect on the exploded edge view that are returned. An edge is included in a windowed view of the graph if
/// it is considered active at any point in the window.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistentGraph(pub Arc<InternalGraph>);

impl Static for PersistentGraph {}

impl From<InternalGraph> for PersistentGraph {
    fn from(value: InternalGraph) -> Self {
        Self(Arc::new(value))
    }
}

impl Display for PersistentGraph {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

fn alive_before<
    A: TimeIndexOps<IndexType = TimeIndexEntry> + ?Sized,
    D: TimeIndexOps<IndexType = TimeIndexEntry> + ?Sized,
>(
    additions: &A,
    deletions: &D,
    t: i64,
) -> bool {
    let first_addition = additions.first();
    let first_deletion = deletions.first();
    let last_addition_before_start = additions.range(i64::MIN..t).last();
    let last_deletion_before_start = deletions.range(i64::MIN..t).last();

    let only_deleted = match (first_addition, first_deletion) {
        (Some(a), Some(d)) => d < a && d >= t.into(),
        (None, Some(d)) => d >= t.into(),
        (Some(_), None) => false,
        (None, None) => false,
    };
    // None is less than any value (see test below)
    only_deleted || last_addition_before_start > last_deletion_before_start
}

fn alive_at<
    A: TimeIndexOps<IndexType = TimeIndexEntry> + ?Sized,
    D: TimeIndexOps<IndexType = TimeIndexEntry> + ?Sized,
>(
    additions: &A,
    deletions: &D,
    t: i64,
) -> bool {
    let deleted_at_start = match (
        deletions.range(t..t.saturating_add(1)).first(),
        additions.range(t..t.saturating_add(1)).first(),
    ) {
        (Some(d), Some(a)) => d < a,
        (Some(_), None) => true,
        _ => false,
    };
    !deleted_at_start && alive_before(additions, deletions, t)
}

fn edge_alive_at_end(e: &dyn EdgeLike, t: i64, layer_ids: &LayerIds) -> bool {
    e.additions_iter(layer_ids)
        .zip_longest(e.deletions_iter(layer_ids))
        .any(|zipped| match zipped {
            EitherOrBoth::Both(additions, deletions) => alive_before(&additions, &deletions, t),
            EitherOrBoth::Left(additions) => additions.active(i64::MIN..t),
            EitherOrBoth::Right(deletions) => deletions.active(t..i64::MAX),
        })
}

fn edge_alive_at_start(e: &dyn EdgeLike, t: i64, layer_ids: &LayerIds) -> bool {
    // The semantics are tricky here, an edge is not alive at the start of the window if the first event at time t is a deletion
    let alive = e
        .additions_iter(layer_ids)
        .zip_longest(e.deletions_iter(layer_ids))
        .any(|zipped| match zipped {
            EitherOrBoth::Both(additions, deletions) => alive_at(&additions, &deletions, t),
            EitherOrBoth::Left(additions) => additions
                .first_t()
                .map(|first_t| first_t <= t)
                .unwrap_or(false),
            EitherOrBoth::Right(deletions) => deletions.first_t() > Some(t),
        });
    alive
}

impl Default for PersistentGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl PersistentGraph {
    pub fn new() -> Self {
        Self(Arc::new(InternalGraph::default()))
    }

    pub fn from_internal_graph(internal_graph: Arc<InternalGraph>) -> Self {
        Self(internal_graph)
    }

    /// Save a graph to a directory
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the directory
    ///
    /// Returns:
    ///
    /// A raphtory graph
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::fs::File;
    /// use raphtory::prelude::*;
    /// let g = Graph::new();
    /// g.add_node(1, 1, NO_PROPS, None).unwrap();
    /// g.save_to_file("path_str").expect("failed to save file");
    /// ```
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), GraphError> {
        MaterializedGraph::from(self.clone()).save_to_file(path)
    }

    /// Load a graph from a directory
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the directory
    ///
    /// Returns:
    ///
    /// A raphtory graph
    ///
    /// # Example
    ///
    /// ```no_run
    /// use raphtory::prelude::*;
    /// let g = Graph::load_from_file("path/to/graph", false);
    /// ```
    pub fn load_from_file<P: AsRef<Path>>(path: P, force: bool) -> Result<Self, GraphError> {
        let g = MaterializedGraph::load_from_file(path, force)?;
        g.into_persistent().ok_or(GraphError::GraphLoadError)
    }

    /// Get event graph
    pub fn event_graph(&self) -> Graph {
        Graph::from_internal_graph(self.0.clone())
    }
}

impl<'graph, G: GraphViewOps<'graph>> PartialEq<G> for PersistentGraph {
    fn eq(&self, other: &G) -> bool {
        graph_equal(self, other)
    }
}

impl Base for PersistentGraph {
    type Base = InternalGraph;
    #[inline(always)]
    fn base(&self) -> &Self::Base {
        &self.0
    }
}

impl InternalMaterialize for PersistentGraph {
    fn new_base_graph(&self, graph: InternalGraph) -> MaterializedGraph {
        MaterializedGraph::PersistentGraph(PersistentGraph(Arc::new(graph)))
    }

    fn include_deletions(&self) -> bool {
        true
    }
}

impl DeletionOps for PersistentGraph {}

impl InheritMutationOps for PersistentGraph {}

impl InheritListOps for PersistentGraph {}

impl InheritCoreOps for PersistentGraph {}

impl InheritCoreDeletionOps for PersistentGraph {}

impl InheritPropertiesOps for PersistentGraph {}

impl InheritLayerOps for PersistentGraph {}

impl InheritEdgeFilterOps for PersistentGraph {}

impl InheritNodeFilterOps for PersistentGraph {}

impl TimeSemantics for PersistentGraph {
    fn node_earliest_time(&self, v: VID) -> Option<i64> {
        self.0.node_earliest_time(v)
    }

    fn node_latest_time(&self, _v: VID) -> Option<i64> {
        Some(i64::MAX)
    }

    fn view_start(&self) -> Option<i64> {
        self.0.view_start()
    }

    fn view_end(&self) -> Option<i64> {
        self.0.view_end()
    }

    fn earliest_time_global(&self) -> Option<i64> {
        self.0.earliest_time_global()
    }

    fn latest_time_global(&self) -> Option<i64> {
        self.0.latest_time_global()
    }

    fn earliest_time_window(&self, start: i64, end: i64) -> Option<i64> {
        self.0.earliest_time_window(start, end)
    }

    fn latest_time_window(&self, start: i64, end: i64) -> Option<i64> {
        self.0.latest_time_window(start, end)
    }

    fn node_earliest_time_window(&self, v: VID, start: i64, end: i64) -> Option<i64> {
        let v = self.core_node_arc(v);
        if v.timestamps().first_t()? <= start {
            Some(v.timestamps().range(start..end).first_t().unwrap_or(start))
        } else {
            None
        }
    }

    fn node_latest_time_window(&self, v: VID, _start: i64, end: i64) -> Option<i64> {
        let v = self.core_node_arc(v);
        if v.timestamps().first_t()? < end {
            Some(end - 1)
        } else {
            None
        }
    }

    fn include_node_window(&self, node: &NodeStore, w: Range<i64>, _layer_ids: &LayerIds) -> bool {
        node.timestamps()
            .first_t()
            .filter(|&t| t <= w.end)
            .is_some()
    }

    fn include_edge_window(&self, edge: &EdgeStore, w: Range<i64>, layer_ids: &LayerIds) -> bool {
        // includes edge if it is alive at the start of the window or added during the window
        edge.active(layer_ids, w.clone()) || edge_alive_at_start(edge, w.start, layer_ids)
    }

    fn node_history(&self, v: VID) -> Vec<i64> {
        self.0.node_history(v)
    }

    fn node_history_window(&self, v: VID, w: Range<i64>) -> Vec<i64> {
        self.0.node_history_window(v, w)
    }

    fn edge_history(&self, e: EdgeRef, layer_ids: LayerIds) -> Vec<i64> {
        self.0.edge_history(e, layer_ids)
    }

    fn edge_history_window(&self, e: EdgeRef, layer_ids: LayerIds, w: Range<i64>) -> Vec<i64> {
        self.0.edge_history_window(e, layer_ids, w)
    }

    fn edge_exploded_count(&self, edge: &EdgeStore, layer_ids: &LayerIds) -> usize {
        match layer_ids {
            LayerIds::None => 0,
            LayerIds::All => (0..self.unfiltered_num_layers())
                .into_par_iter()
                .map(|id| self.edge_exploded_count(edge, &LayerIds::One(id)))
                .sum(),
            LayerIds::One(id) => {
                let additions = edge.additions.get(*id).unwrap_or(&TimeIndex::Empty);
                let deletions = edge.deletions.get(*id).unwrap_or(&TimeIndex::Empty);
                let a_first = additions.first().unwrap_or(TimeIndexEntry::MAX);
                let d_first = deletions.first().unwrap_or(TimeIndexEntry::MAX);
                if d_first < a_first {
                    additions.len() + 1
                } else {
                    additions.len()
                }
            }
            LayerIds::Multiple(layers) => layers
                .par_iter()
                .map(|id| self.edge_exploded_count(edge, &LayerIds::One(*id)))
                .sum(),
        }
    }

    fn edge_exploded_count_window(
        &self,
        edge: &EdgeStore,
        layer_ids: &LayerIds,
        w: Range<i64>,
    ) -> usize {
        match layer_ids {
            LayerIds::None => 0,
            LayerIds::All => (0..self.unfiltered_num_layers())
                .into_par_iter()
                .map(|id| self.edge_exploded_count_window(edge, &LayerIds::One(id), w.clone()))
                .sum(),
            LayerIds::One(id) => {
                let additions = edge.additions.get(*id).unwrap_or(&TimeIndex::Empty);
                let deletions = edge.deletions.get(*id).unwrap_or(&TimeIndex::Empty);
                let mut len = additions.range(w.clone()).len();
                if alive_at(additions, deletions, w.start) {
                    len += 1
                }
                len
            }
            LayerIds::Multiple(layers) => layers
                .par_iter()
                .map(|id| self.edge_exploded_count(edge, &LayerIds::One(*id)))
                .sum(),
        }
    }

    fn edge_exploded(&self, e: EdgeRef, layer_ids: &LayerIds) -> BoxedIter<EdgeRef> {
        let edge = self.0.core_edge_arc(e.pid());

        let alive_layers: Vec<_> = edge
            .updates_iter(layer_ids)
            .filter_map(
                |(l, additions, deletions)| match (additions.first(), deletions.first()) {
                    (Some(a), Some(d)) => (d < a).then_some(l),
                    (None, Some(_)) => Some(l),
                    _ => None,
                },
            )
            .collect();
        alive_layers
            .into_iter()
            .map(move |l| e.at(i64::MIN.into()).at_layer(l))
            .chain(self.0.edge_exploded(e, layer_ids))
            .into_dyn_boxed()
    }

    fn edge_layers(&self, e: EdgeRef, layer_ids: &LayerIds) -> BoxedIter<EdgeRef> {
        self.0.edge_layers(e, layer_ids)
    }

    fn edge_window_exploded(
        &self,
        e: EdgeRef,
        w: Range<i64>,
        layer_ids: &LayerIds,
    ) -> BoxedIter<EdgeRef> {
        if w.end <= w.start {
            return Box::new(iter::empty());
        }
        let edge = self.0.core_edge_arc(e.pid());

        let alive_layers: Vec<_> = edge
            .updates_iter(layer_ids)
            .filter_map(|(l, additions, deletions)| {
                alive_at(additions, deletions, w.start).then_some(l)
            })
            .collect();
        alive_layers
            .into_iter()
            .map(move |l| e.at(w.start.into()).at_layer(l))
            .chain(self.0.edge_window_exploded(e, w, layer_ids))
            .into_dyn_boxed()
    }

    fn edge_window_layers(
        &self,
        e: EdgeRef,
        w: Range<i64>,
        layer_ids: &LayerIds,
    ) -> BoxedIter<EdgeRef> {
        let g = self.clone();
        let edge = self.core_edge_arc(e.pid());
        Box::new(g.edge_layers(e, layer_ids).filter(move |&e| {
            g.include_edge_window(&edge, w.clone(), &LayerIds::One(*e.layer().unwrap()))
        }))
    }

    fn edge_earliest_time(&self, e: EdgeRef, layer_ids: &LayerIds) -> Option<i64> {
        e.time().map(|ti| ti.t()).or_else(|| {
            let entry = self.core_edge_arc(e.pid());
            if edge_alive_at_start(entry.deref(), i64::MIN, layer_ids) {
                Some(i64::MIN)
            } else {
                self.edge_additions(e, layer_ids.clone())
                    .first()
                    .map(|ti| ti.t())
            }
        })
    }

    fn edge_earliest_time_window(
        &self,
        e: EdgeRef,
        w: Range<i64>,
        layer_ids: &LayerIds,
    ) -> Option<i64> {
        let entry = self.core_edge_arc(e.pid());
        if edge_alive_at_start(entry.deref(), w.start, layer_ids) {
            Some(w.start)
        } else {
            self.edge_additions(e, layer_ids.clone()).range(w).first_t()
        }
    }

    fn edge_latest_time(&self, e: EdgeRef, layer_ids: &LayerIds) -> Option<i64> {
        match e.time().map(|ti| ti.t()) {
            Some(t) => Some(min(
                self.edge_additions(e, layer_ids.clone())
                    .range(t.saturating_add(1)..i64::MAX)
                    .first_t()
                    .unwrap_or(i64::MAX),
                self.edge_deletions(e, layer_ids.clone())
                    .range(t.saturating_add(1)..i64::MAX)
                    .first_t()
                    .unwrap_or(i64::MAX),
            )),
            None => {
                let entry = self.core_edge_arc(e.pid());
                if edge_alive_at_end(entry.deref(), i64::MAX, layer_ids) {
                    Some(i64::MAX)
                } else {
                    self.edge_deletions(e, layer_ids.clone()).last_t()
                }
            }
        }
    }

    fn edge_latest_time_window(
        &self,
        e: EdgeRef,
        w: Range<i64>,
        layer_ids: &LayerIds,
    ) -> Option<i64> {
        match e.time().map(|ti| ti.t()) {
            Some(t) => Some(min(
                self.edge_additions(e, layer_ids.clone())
                    .range(t.saturating_add(1)..w.end)
                    .first_t()
                    .unwrap_or(w.end - 1),
                self.edge_deletions(e, layer_ids.clone())
                    .range(t.saturating_add(1)..w.end)
                    .first_t()
                    .unwrap_or(w.end - 1),
            )),
            None => {
                let entry = self.core_edge_arc(e.pid());
                if edge_alive_at_end(entry.deref(), w.end, layer_ids) {
                    return Some(w.end - 1);
                }
                entry
                    .updates_iter(layer_ids)
                    .flat_map(|(_, additions, deletions)| {
                        let last_deletion = deletions.range(w.clone()).last()?;
                        if last_deletion.t() > w.start || additions.active(w.clone()) {
                            Some(last_deletion.t())
                        } else {
                            None
                        }
                    })
                    .max()
            }
        }
    }

    fn edge_deletion_history(&self, e: EdgeRef, layer_ids: &LayerIds) -> Vec<i64> {
        self.edge_deletions(e, layer_ids.clone()).iter_t().collect()
    }

    fn edge_deletion_history_window(
        &self,
        e: EdgeRef,
        w: Range<i64>,
        layer_ids: &LayerIds,
    ) -> Vec<i64> {
        self.edge_deletions(e, layer_ids.clone())
            .range(w)
            .iter_t()
            .collect()
    }

    fn edge_is_valid(&self, e: EdgeRef, layer_ids: &LayerIds) -> bool {
        let edge = self.0.core_edge_arc(e.pid());
        let res = edge
            .updates_iter(layer_ids)
            .any(|(_, additions, deletions)| additions.last() > deletions.last());
        res
    }

    fn edge_is_valid_at_end(&self, e: EdgeRef, layer_ids: &LayerIds, end: i64) -> bool {
        let edge = self.0.core_edge_arc(e.pid());
        edge_alive_at_end(edge.deref(), end, &layer_ids)
    }

    #[inline]
    fn has_temporal_prop(&self, prop_id: usize) -> bool {
        self.0.has_temporal_prop(prop_id)
    }

    fn temporal_prop_vec(&self, prop_id: usize) -> Vec<(i64, Prop)> {
        self.0.temporal_prop_vec(prop_id)
    }

    #[inline]
    fn has_temporal_prop_window(&self, prop_id: usize, w: Range<i64>) -> bool {
        self.0.has_temporal_prop_window(prop_id, w)
    }

    fn temporal_prop_vec_window(&self, prop_id: usize, start: i64, end: i64) -> Vec<(i64, Prop)> {
        self.0.temporal_prop_vec_window(prop_id, start, end)
    }

    #[inline]
    fn has_temporal_node_prop(&self, v: VID, prop_id: usize) -> bool {
        self.0.has_temporal_node_prop(v, prop_id)
    }

    fn temporal_node_prop_vec(&self, v: VID, prop_id: usize) -> Vec<(i64, Prop)> {
        self.0.temporal_node_prop_vec(v, prop_id)
    }

    fn has_temporal_node_prop_window(&self, v: VID, prop_id: usize, w: Range<i64>) -> bool {
        self.0
            .has_temporal_node_prop_window(v, prop_id, i64::MIN..w.end)
    }

    fn temporal_node_prop_vec_window(
        &self,
        v: VID,
        prop_id: usize,
        start: i64,
        end: i64,
    ) -> Vec<(i64, Prop)> {
        let prop = self.temporal_node_prop(v, prop_id);
        match prop {
            Some(p) => p
                .last_before(start.saturating_add(1))
                .into_iter()
                .map(|(_, v)| (start, v))
                .chain(p.iter_window_t(start.saturating_add(1)..end))
                .collect(),
            None => Default::default(),
        }
    }

    fn has_temporal_edge_prop_window(
        &self,
        e: EdgeRef,
        prop_id: usize,
        w: Range<i64>,
        layer_ids: LayerIds,
    ) -> bool {
        let entry = self.core_edge_arc(e.pid());

        if entry.has_temporal_prop(&layer_ids, prop_id) {
            // if property was added at any point since the last deletion, it is still there,
            // if deleted at the start of the window, we still need to check for any additions
            // that happened at the same time
            let search_start = min(
                entry
                    .last_deletion_before(&layer_ids, w.start.saturating_add(1))
                    .unwrap_or(TimeIndexEntry::MIN),
                TimeIndexEntry::start(w.start),
            );
            let search_end = TimeIndexEntry::start(w.end);
            match layer_ids {
                LayerIds::None => false,
                LayerIds::All => entry.layer_ids_iter().any(|id| {
                    entry
                        .temporal_prop_layer(id, prop_id)
                        .filter(|prop| prop.iter_window(search_start..search_end).next().is_some())
                        .is_some()
                }),
                LayerIds::One(id) => entry
                    .temporal_prop_layer(id, prop_id)
                    .filter(|prop| prop.iter_window(search_start..search_end).next().is_some())
                    .is_some(),
                LayerIds::Multiple(ids) => ids.iter().any(|&id| {
                    entry
                        .temporal_prop_layer(id, prop_id)
                        .filter(|prop| prop.iter_window(search_start..search_end).next().is_some())
                        .is_some()
                }),
            }
        } else {
            false
        }
    }

    fn temporal_edge_prop_vec_window(
        &self,
        e: EdgeRef,
        prop_id: usize,
        start: i64,
        end: i64,
        layer_ids: LayerIds,
    ) -> Vec<(i64, Prop)> {
        let prop = self.temporal_edge_prop(e, prop_id, layer_ids.clone());
        match prop {
            Some(p) => {
                let entry = self.core_edge_arc(e.pid());
                if edge_alive_at_start(entry.deref(), start, &layer_ids) {
                    p.last_before(start.saturating_add(1))
                        .into_iter()
                        .map(|(_, v)| (start, v))
                        .chain(p.iter_window(start.saturating_add(1)..end))
                        .collect()
                } else {
                    p.iter_window(start..end).collect()
                }
            }
            None => Default::default(),
        }
    }

    fn has_temporal_edge_prop(&self, e: EdgeRef, prop_id: usize, layer_ids: LayerIds) -> bool {
        self.0.has_temporal_edge_prop(e, prop_id, layer_ids)
    }

    fn temporal_edge_prop_vec(
        &self,
        e: EdgeRef,
        prop_id: usize,
        layer_ids: LayerIds,
    ) -> Vec<(i64, Prop)> {
        self.0.temporal_edge_prop_vec(e, prop_id, layer_ids)
    }
}

#[cfg(test)]
mod test_deletions {
    use crate::{
        db::{
            api::view::time::internal::InternalTimeOps,
            graph::{
                edge::EdgeView, graph::assert_graph_equal, views::deletion_graph::PersistentGraph,
            },
        },
        prelude::*,
    };
    use itertools::Itertools;

    #[test]
    fn test_nodes() {
        let g = PersistentGraph::new();

        g.add_edge(0, 1, 2, [("added", Prop::I64(0))], Some("assigned"))
            .unwrap();
        g.add_edge(1, 1, 3, [("added", Prop::I64(0))], Some("assigned"))
            .unwrap();
        g.add_edge(2, 4, 2, [("added", Prop::I64(0))], Some("has"))
            .unwrap();
        g.add_edge(3, 4, 2, [("added", Prop::I64(0))], Some("has"))
            .unwrap();
        g.add_edge(4, 5, 2, [("added", Prop::I64(0))], Some("blocks"))
            .unwrap();
        g.add_edge(5, 4, 5, [("added", Prop::I64(0))], Some("has"))
            .unwrap();
        g.add_edge(6, 6, 5, [("added", Prop::I64(0))], Some("assigned"))
            .unwrap();

        let nodes = g
            .window(0, 1701786285758)
            .layers(vec!["assigned", "has", "blocks"])
            .unwrap()
            .nodes()
            .into_iter()
            .map(|vv| vv.name())
            .collect_vec();

        assert_eq!(nodes, vec!["1", "2", "3", "4", "5", "6"]);

        let nodes = g
            .at(1701786285758)
            .layers(vec!["assigned", "has", "blocks"])
            .unwrap()
            .nodes()
            .into_iter()
            .map(|vv| vv.name())
            .collect_vec();

        assert_eq!(nodes, vec!["1", "2", "3", "4", "5", "6"]);
    }

    #[test]
    fn test_edge_deletions() {
        let g = PersistentGraph::new();

        g.add_edge(0, 0, 1, [("added", Prop::I64(0))], None)
            .unwrap();
        g.delete_edge(10, 0, 1, None).unwrap();

        assert_eq!(g.edges().id().collect::<Vec<_>>(), vec![(0, 1)]);

        assert_eq!(
            g.window(1, 2).edges().id().collect::<Vec<_>>(),
            vec![(0, 1)]
        );

        assert_eq!(g.window(1, 2).count_edges(), 1);

        assert_eq!(g.window(11, 12).count_edges(), 0);

        assert_eq!(
            g.window(1, 2)
                .edge(0, 1)
                .unwrap()
                .properties()
                .get("added")
                .unwrap_i64(),
            0
        );

        assert!(g.window(11, 12).edge(0, 1).is_none());

        assert_eq!(
            g.window(1, 2)
                .edge(0, 1)
                .unwrap()
                .properties()
                .temporal()
                .get("added")
                .unwrap()
                .iter()
                .collect_vec(),
            vec![(1, Prop::I64(0))]
        );

        assert_eq!(g.window(1, 2).node(0).unwrap().out_degree(), 1)
    }

    #[test]
    fn test_window_semantics() {
        let g = PersistentGraph::new();
        g.add_edge(1, 1, 2, [("test", "test")], None).unwrap();
        g.delete_edge(10, 1, 2, None).unwrap();

        assert_eq!(g.count_edges(), 1);

        assert_eq!(g.at(12).count_edges(), 0);
        assert_eq!(g.at(11).count_edges(), 0);
        assert_eq!(g.at(10).count_edges(), 0);
        assert_eq!(g.at(9).count_edges(), 1);
        assert_eq!(g.window(5, 9).count_edges(), 1);
        assert_eq!(g.window(5, 10).count_edges(), 1);
        assert_eq!(g.window(5, 11).count_edges(), 1);
        assert_eq!(g.window(10, 12).count_edges(), 0);
        assert_eq!(g.before(10).count_edges(), 1);
        assert_eq!(g.after(10).count_edges(), 0);
    }

    #[test]
    fn test_timestamps() {
        let g = PersistentGraph::new();
        let e = g.add_edge(1, 1, 2, [("test", "test")], None).unwrap();
        assert_eq!(e.earliest_time().unwrap(), 1);
        assert_eq!(e.latest_time(), Some(i64::MAX));
        g.delete_edge(10, 1, 2, None).unwrap();
        assert_eq!(e.latest_time().unwrap(), 10);

        g.delete_edge(10, 3, 4, None).unwrap();
        let e = g.edge(3, 4).unwrap();
        assert_eq!(e.earliest_time(), Some(i64::MIN));
        assert_eq!(e.latest_time().unwrap(), 10);
        g.add_edge(1, 3, 4, [("test", "test")], None).unwrap();
        assert_eq!(e.latest_time().unwrap(), 10);
        assert_eq!(e.earliest_time().unwrap(), 1);
    }

    #[test]
    fn test_materialize_only_deletion() {
        let g = PersistentGraph::new();
        g.delete_edge(1, 1, 2, None).unwrap();
        g.add_edge(2, 1, 2, NO_PROPS, None).unwrap();
        g.delete_edge(5, 1, 2, None).unwrap();
        g.delete_edge(10, 1, 2, None).unwrap();
        assert_eq!(
            g.window(0, 11).count_temporal_edges(),
            g.count_temporal_edges()
        );
        assert_graph_equal(&g.materialize().unwrap(), &g);
    }

    #[test]
    fn test_materialize_window() {
        let g = PersistentGraph::new();
        g.add_edge(0, 1, 2, NO_PROPS, None).unwrap();
        g.delete_edge(10, 1, 2, None).unwrap();

        let gm = g
            .window(3, 5)
            .materialize()
            .unwrap()
            .into_persistent()
            .unwrap();
        assert_graph_equal(&gm, &g.window(3, 5))
    }

    #[test]
    fn test_exploded_latest_time() {
        let g = PersistentGraph::new();
        let e = g.add_edge(0, 1, 2, NO_PROPS, None).unwrap();
        g.delete_edge(10, 1, 2, None).unwrap();
        assert_eq!(e.latest_time(), Some(10));
        assert_eq!(e.explode().latest_time().collect_vec(), vec![Some(10)]);
    }

    #[test]
    fn test_exploded_window() {
        let g = PersistentGraph::new();
        let e = g.add_edge(0, 1, 2, NO_PROPS, None).unwrap();
        for t in [5, 10, 15] {
            e.add_updates(t, NO_PROPS, None).unwrap();
        }
        assert_eq!(
            e.after(2).explode().time().flatten().collect_vec(),
            [3, 5, 10, 15]
        );
    }

    #[test]
    fn test_edge_properties() {
        let g = PersistentGraph::new();
        let e = g.add_edge(0, 1, 2, [("test", "test")], None).unwrap();
        assert_eq!(e.properties().get("test").unwrap_str(), "test");
        e.delete(10, None).unwrap();
        assert_eq!(e.properties().get("test").unwrap_str(), "test");
        assert_eq!(e.at(10).properties().get("test"), None);
        e.add_updates(11, [("test", "test11")], None).unwrap();
        assert_eq!(
            e.window(10, 12).properties().get("test").unwrap_str(),
            "test11"
        );
        assert_eq!(
            e.window(5, 12)
                .properties()
                .temporal()
                .get("test")
                .unwrap()
                .iter()
                .collect_vec(),
            vec![(5, Prop::str("test")), (11i64, Prop::str("test11"))],
        );
    }

    #[test]
    fn test_edge_history() {
        let g = PersistentGraph::new();
        let e = g.add_edge(0, 1, 2, NO_PROPS, None).unwrap();
        e.delete(5, None).unwrap();
        e.add_updates(10, NO_PROPS, None).unwrap();
        assert_eq!(e.history(), [0, 10]);
        assert_eq!(e.after(1).history(), [10]);
        assert!(e.window(1, 4).history().is_empty());

        // exploded edge still exists
        assert_eq!(
            e.window(1, 4)
                .explode()
                .earliest_time()
                .flatten()
                .collect_vec(),
            [1]
        );
    }

    #[test]
    fn test_ordering_of_addition_and_deletion() {
        let g = PersistentGraph::new();

        //deletion before addition (edge exists from (-inf,1) and (1, inf)
        g.delete_edge(1, 1, 2, None).unwrap();
        let e_1_2 = g.add_edge(1, 1, 2, [("test", "test")], None).unwrap();

        //deletion after addition (edge exists only at 2)
        let e_3_4 = g.add_edge(2, 3, 4, [("test", "test")], None).unwrap();
        g.delete_edge(2, 3, 4, None).unwrap();

        assert_eq!(e_1_2.at(0).properties().get("test"), None);
        assert_eq!(e_1_2.at(1).properties().get("test").unwrap_str(), "test");
        assert_eq!(e_1_2.at(2).properties().get("test").unwrap_str(), "test");

        assert_eq!(e_3_4.at(0).properties().get("test"), None);
        assert_eq!(e_3_4.at(2).properties().get("test").unwrap_str(), "test");
        assert_eq!(e_3_4.at(3).properties().get("test"), None);

        assert!(g.window(0, 1).has_edge(1, 2));
        assert!(!g.window(0, 2).has_edge(3, 4));
        assert!(g.window(1, 2).has_edge(1, 2));
        assert!(g.window(2, 3).has_edge(3, 4));
        assert!(!g.window(3, 4).has_edge(3, 4));
    }

    #[test]
    fn test_deletions() {
        let edges = [
            (1, 1, 2),
            (2, 1, 3),
            (-1, 2, 1),
            (0, 1, 1),
            (7, 3, 2),
            (1, 1, 1),
        ];
        let g = PersistentGraph::new();
        for (t, s, d) in edges.iter() {
            g.add_edge(*t, *s, *d, NO_PROPS, None).unwrap();
        }
        g.delete_edge(10, edges[0].1, edges[0].2, None).unwrap();

        for (t, s, d) in &edges {
            assert!(g.at(*t).has_edge(*s, *d));
        }
        assert!(!g.after(10).has_edge(edges[0].1, edges[0].2));
        for (_, s, d) in &edges[1..] {
            assert!(g.after(10).has_edge(*s, *d));
        }
        assert_eq!(
            g.edge(edges[0].1, edges[0].2)
                .unwrap()
                .explode()
                .latest_time()
                .collect_vec(),
            [Some(10)]
        );
    }

    #[test]
    fn test_deletion_multiple_layers() {
        let g = PersistentGraph::new();

        g.add_edge(1, 1, 2, NO_PROPS, Some("1")).unwrap();
        g.delete_edge(2, 1, 2, Some("2")).unwrap();
        g.delete_edge(10, 1, 2, Some("1")).unwrap();
        g.add_edge(10, 1, 2, NO_PROPS, Some("2")).unwrap();

        let e = g.edge(1, 2).unwrap();
        let e_layer_1 = e.layers("1").unwrap();
        let e_layer_2 = e.layers("2").unwrap();

        for t in 0..11 {
            assert!(g.at(t).has_edge(1, 2));
        }

        assert!(e.is_valid());
        assert!(!e_layer_1.is_valid());
        assert!(e_layer_2.is_valid());
        assert!(!e_layer_1.at(10).is_valid());
        for t in 0..11 {
            assert!(e.at(t).is_valid());
        }
    }

    fn check_valid<'graph, G: GraphViewOps<'graph>, GH: GraphViewOps<'graph>>(e: &EdgeView<G, GH>) {
        assert!(e.is_valid());
        assert!(!e.is_deleted());
        assert!(e.graph.has_edge(e.src(), e.dst()));
        assert!(e.graph.edge(e.src(), e.dst()).is_some());
    }

    fn check_deleted<'graph, G: GraphViewOps<'graph>, GH: GraphViewOps<'graph>>(
        e: &EdgeView<G, GH>,
    ) {
        assert!(!e.is_valid());
        assert!(e.is_deleted());
        let t = e.latest_time().unwrap_or(i64::MAX);
        let g = e.graph.at(t); // latest view of the graph
        assert!(!g.has_edge(e.src(), e.dst()));
        assert!(g.edge(e.src(), e.dst()).is_none());
    }

    #[test]
    fn test_edge_is_valid() {
        let g = PersistentGraph::new();

        g.add_edge(1, 1, 2, NO_PROPS, None).unwrap();
        let e = g.edge(1, 2).unwrap();
        check_deleted(&e.before(1));
        check_valid(&e.after(1));
        check_valid(&e);

        g.add_edge(2, 1, 2, NO_PROPS, Some("1")).unwrap();
        check_valid(&e);

        g.delete_edge(3, 1, 2, Some("1")).unwrap();
        check_valid(&e);
        check_deleted(&e.layers("1").unwrap());
        check_deleted(&e.layers("1").unwrap().at(3));
        check_deleted(&e.layers("1").unwrap().after(3));
        check_valid(&e.layers("1").unwrap().before(3));
        check_valid(&e.default_layer());

        g.delete_edge(4, 1, 2, None).unwrap();
        check_deleted(&e);
        check_deleted(&e.layers("1").unwrap());
        check_deleted(&e.default_layer());

        g.add_edge(5, 1, 2, NO_PROPS, None).unwrap();
        check_valid(&e);
        check_valid(&e.default_layer());
        check_deleted(&e.layers("1").unwrap());
    }

    #[test]
    fn test_explode_multiple_layers() {
        let g = PersistentGraph::new();
        g.delete_edge(1, 1, 2, Some("1")).unwrap();
        g.delete_edge(2, 1, 2, Some("2")).unwrap();
        g.delete_edge(3, 1, 2, Some("3")).unwrap();

        let e = g.edge(1, 2).unwrap();
        assert_eq!(e.explode().iter().count(), 3);
        assert_eq!(e.before(4).explode().iter().count(), 3);
        assert_eq!(e.window(2, 3).explode().iter().count(), 1);
    }

    #[test]
    fn test_edge_latest_time() {
        let g = PersistentGraph::new();
        let e = g.add_edge(0, 1, 2, NO_PROPS, None).unwrap();
        e.delete(2, None).unwrap();
        assert_eq!(e.at(2).earliest_time(), None);
        assert_eq!(e.at(2).latest_time(), None);
        assert!(e.at(2).is_deleted());
        assert_eq!(e.latest_time(), Some(2));
        e.add_updates(4, NO_PROPS, None).unwrap();
        assert_eq!(e.latest_time(), Some(i64::MAX));

        assert_eq!(e.window(0, 3).latest_time(), Some(2));
    }

    #[test]
    fn test_view_start_end() {
        let g = PersistentGraph::new();
        let e = g.add_edge(0, 1, 2, NO_PROPS, None).unwrap();
        assert_eq!(g.start(), None);
        assert_eq!(g.timeline_start(), Some(0));
        assert_eq!(g.end(), None);
        assert_eq!(g.timeline_end(), Some(1));
        e.delete(2, None).unwrap();
        assert_eq!(g.timeline_start(), Some(0));
        assert_eq!(g.timeline_end(), Some(3));
        let w = g.window(g.timeline_start().unwrap(), g.timeline_end().unwrap());
        assert!(g.has_edge(1, 2));
        assert!(w.has_edge(1, 2));
        assert_eq!(w.start(), Some(0));
        assert_eq!(w.timeline_start(), Some(0));
        assert_eq!(w.end(), Some(3));
        assert_eq!(w.timeline_end(), Some(3));

        e.add_updates(4, NO_PROPS, None).unwrap();
        assert_eq!(g.timeline_start(), Some(0));
        assert_eq!(g.timeline_end(), Some(5));
    }

    #[test]
    fn test_node_property_semantics() {
        let g = PersistentGraph::new();
        let _v = g
            .add_node(1, 1, [("test_prop", "test value")], None)
            .unwrap();
        let v = g
            .add_node(11, 1, [("test_prop", "test value 2")], None)
            .unwrap();
        let v_from_graph = g.at(10).node(1).unwrap();
        assert_eq!(v.properties().get("test_prop").unwrap_str(), "test value 2");
        assert_eq!(
            v.at(10).properties().get("test_prop").unwrap_str(),
            "test value"
        );
        assert_eq!(
            v.at(11).properties().get("test_prop").unwrap_str(),
            "test value 2"
        );
        assert_eq!(
            v_from_graph.properties().get("test_prop").unwrap_str(),
            "test value"
        );

        assert_eq!(
            v.before(11).properties().get("test_prop").unwrap_str(),
            "test value"
        );

        assert_eq!(
            v.properties()
                .temporal()
                .get("test_prop")
                .unwrap()
                .history(),
            [1, 11]
        );
        assert_eq!(
            v_from_graph
                .properties()
                .temporal()
                .get("test_prop")
                .unwrap()
                .history(),
            [10]
        );

        assert_eq!(v_from_graph.earliest_time(), Some(10));
        assert_eq!(v.earliest_time(), Some(1));
        assert_eq!(v.at(10).earliest_time(), Some(10));
        assert_eq!(v.at(10).latest_time(), Some(10));
        assert_eq!(v.latest_time(), Some(i64::MAX));
    }

    #[test]
    fn test_jira() {
        let g = PersistentGraph::new();

        g.add_edge(0, 1, 2, [("added", Prop::I64(0))], Some("assigned"))
            .unwrap();
        g.add_edge(1, 1, 3, [("added", Prop::I64(0))], Some("assigned"))
            .unwrap();
        g.add_edge(2, 4, 2, [("added", Prop::I64(0))], Some("has"))
            .unwrap();
        g.add_edge(3, 4, 2, [("added", Prop::I64(0))], Some("has"))
            .unwrap();
        g.add_edge(4, 5, 2, [("added", Prop::I64(0))], Some("blocks"))
            .unwrap();
        g.add_edge(5, 4, 5, [("added", Prop::I64(0))], Some("has"))
            .unwrap();
        g.add_edge(6, 6, 5, [("added", Prop::I64(0))], Some("assigned"))
            .unwrap();

        let nodes = g
            .window(0, 1701786285758)
            .layers(vec!["assigned", "has", "blocks"])
            .unwrap()
            .edges()
            .into_iter()
            .map(|vv| vv.id())
            .collect_vec();

        println!("windowed edges = {:?}", nodes);

        let nodes = g
            .window(0, 1701786285758)
            .layers(vec!["assigned", "has", "blocks"])
            .unwrap()
            .nodes()
            .into_iter()
            .map(|vv| vv.name())
            .collect_vec();

        println!("windowed nodes = {:?}", nodes);

        let nodes = g
            .at(1701786285758)
            .layers(vec!["assigned", "has", "blocks"])
            .unwrap()
            .edges()
            .into_iter()
            .map(|vv| vv.id())
            .collect_vec();
        println!("at edges = {:?}", nodes);

        let nodes = g
            .at(1701786285758)
            .layers(vec!["assigned", "has", "blocks"])
            .unwrap()
            .nodes()
            .into_iter()
            .map(|vv| vv.id())
            .collect_vec();

        println!("at nodes = {:?}", nodes);
        // assert_eq!(g.window(1, 2).node(0).unwrap().out_degree(), 1)
    }

    #[test]
    fn test_event_graph() {
        let pg = PersistentGraph::new();
        pg.add_edge(0, 0, 1, [("added", Prop::I64(0))], None)
            .unwrap();
        pg.delete_edge(10, 0, 1, None).unwrap();
        assert_eq!(pg.edges().id().collect::<Vec<_>>(), vec![(0, 1)]);

        let g = pg.event_graph();
        assert_eq!(g.edges().id().collect::<Vec<_>>(), vec![(0, 1)]);
    }
}
