extern crate core;

use py_raphtory::algorithms::*;
use py_raphtory::graph::PyGraph;
use py_raphtory::graph_gen::*;
use py_raphtory::graph_loader::*;
use pyo3::prelude::*;
use py_raphtory::edge::{PyEdge, PyEdges};
use py_raphtory::vertex::{PyVertex, PyVertices};

/// Raphtory graph analytics library
#[pymodule]
fn raphtory(py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyGraph>()?;

    let algorithm_module = PyModule::new(py, "algorithms")?;
    algorithm_module.add_function(wrap_pyfunction!(global_reciprocity, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(all_local_reciprocity, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(triplet_count, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(
        global_clustering_coefficient,
        algorithm_module
    )?)?;
    algorithm_module.add_function(wrap_pyfunction!(local_triangle_count, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(generic_taint, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(
        local_clustering_coefficient,
        algorithm_module
    )?)?;
    algorithm_module.add_function(wrap_pyfunction!(average_degree, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(directed_graph_density, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(max_out_degree, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(max_in_degree, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(min_out_degree, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(min_in_degree, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(pagerank, algorithm_module)?)?;
    algorithm_module.add_function(wrap_pyfunction!(
        weakly_connected_components,
        algorithm_module
    )?)?;
    algorithm_module.add_function(wrap_pyfunction!(
        temporal_three_node_motif,
        algorithm_module
    )?)?;
    algorithm_module.add_function(wrap_pyfunction!(
        global_temporal_three_node_motif,
        algorithm_module
    )?)?;
    algorithm_module.add_function(wrap_pyfunction!(
        global_temporal_three_node_motif_from_local,
        algorithm_module
    )?)?;

    m.add_submodule(algorithm_module)?;
    let graph_loader_module = PyModule::new(py, "graph_loader")?;
    graph_loader_module.add_function(wrap_pyfunction!(lotr_graph, graph_loader_module)?)?;
    graph_loader_module.add_function(wrap_pyfunction!(
        reddit_hyperlink_graph,
        graph_loader_module
    )?)?;
    graph_loader_module.add_function(wrap_pyfunction!(neo4j_movie_graph, graph_loader_module)?)?;
    graph_loader_module.add_function(wrap_pyfunction!(stable_coin_graph, graph_loader_module)?)?;
    m.add_submodule(graph_loader_module)?;

    let graph_gen_module = PyModule::new(py, "graph_gen")?;
    graph_gen_module.add_function(wrap_pyfunction!(random_attachment, graph_gen_module)?)?;
    graph_gen_module.add_function(wrap_pyfunction!(
        ba_preferential_attachment,
        graph_gen_module
    )?)?;
    m.add_submodule(graph_gen_module)?;

    m.add_class::<PyVertex>()?;
    m.add_class::<PyVertices>()?;
    m.add_class::<PyEdge>()?;
    m.add_class::<PyEdges>()?;

    Ok(())
}
