use crate::{
    core::{utils::errors::GraphError, Prop},
    db::api::{storage::graph::storage_ops::GraphStorage, view::internal::CoreGraphOps},
    io::arrow::{dataframe::*, df_loaders::*},
    python::graph::io::*,
};
use polars_arrow::{array::Array, ffi};
use pyo3::{
    ffi::Py_uintptr_t,
    prelude::*,
    types::{IntoPyDict, PyDict},
};
use std::collections::HashMap;

pub fn load_nodes_from_pandas(
    graph: &GraphStorage,
    df: &PyAny,
    time: &str,
    id: &str,
    node_type: Option<&str>,
    node_type_col: Option<&str>,
    properties: Option<&[&str]>,
    constant_properties: Option<&[&str]>,
    shared_constant_properties: Option<&HashMap<String, Prop>>,
) -> Result<(), GraphError> {
    Python::with_gil(|py| {
        let mut cols_to_check = vec![id, time];
        cols_to_check.extend(properties.unwrap_or(&Vec::new()));
        cols_to_check.extend(constant_properties.unwrap_or(&Vec::new()));
        if let Some(ref node_type_col) = node_type_col {
            cols_to_check.push(node_type_col.as_ref());
        }

        let df_view = process_pandas_py_df(df, py, cols_to_check.clone())?;
        df_view.check_cols_exist(&cols_to_check)?;
        load_nodes_from_df(
            df_view,
            time,
            id,
            properties,
            constant_properties,
            shared_constant_properties,
            node_type,
            node_type_col,
            graph,
        )
        .map_err(|e| GraphLoadException::new_err(format!("{:?}", e)))?;
        Ok::<(), PyErr>(())
    })
    .map_err(|e| GraphError::LoadFailure(format!("Failed to load graph {e:?}")))?;
    Ok(())
}

pub fn load_edges_from_pandas(
    graph: &GraphStorage,
    df: &PyAny,
    time: &str,
    src: &str,
    dst: &str,
    properties: Option<&[&str]>,
    constant_properties: Option<&[&str]>,
    shared_constant_properties: Option<&HashMap<String, Prop>>,
    layer: Option<&str>,
    layer_col: Option<&str>,
) -> Result<(), GraphError> {
    Python::with_gil(|py| {
        let mut cols_to_check = vec![src, dst, time];
        cols_to_check.extend(properties.unwrap_or(&Vec::new()));
        cols_to_check.extend(constant_properties.unwrap_or(&Vec::new()));
        if let Some(ref layer_col) = layer_col {
            cols_to_check.push(layer_col.as_ref());
        }

        let df_view = process_pandas_py_df(df, py, cols_to_check.clone())?;
        df_view.check_cols_exist(&cols_to_check)?;
        load_edges_from_df(
            df_view,
            time,
            src,
            dst,
            properties,
            constant_properties,
            shared_constant_properties,
            layer,
            layer_col,
            graph,
        )
        .map_err(|e| GraphLoadException::new_err(format!("{:?}", e)))?;
        Ok::<(), PyErr>(())
    })
    .map_err(|e| GraphError::LoadFailure(format!("Failed to load graph {e:?}")))?;
    Ok(())
}

pub fn load_node_props_from_pandas(
    graph: &GraphStorage,
    df: &PyAny,
    id: &str,
    node_type: Option<&str>,
    node_type_col: Option<&str>,
    constant_properties: Option<&[&str]>,
    shared_constant_properties: Option<&HashMap<String, Prop>>,
) -> Result<(), GraphError> {
    Python::with_gil(|py| {
        let mut cols_to_check = vec![id];
        cols_to_check.extend(constant_properties.unwrap_or(&Vec::new()));
        if let Some(ref node_type_col) = node_type_col {
            cols_to_check.push(node_type_col.as_ref());
        }
        let df_view = process_pandas_py_df(df, py, cols_to_check.clone())?;
        df_view.check_cols_exist(&cols_to_check)?;
        load_node_props_from_df(
            df_view,
            id,
            node_type,
            node_type_col,
            constant_properties,
            shared_constant_properties,
            graph,
        )
        .map_err(|e| GraphLoadException::new_err(format!("{:?}", e)))?;
        Ok::<(), PyErr>(())
    })
    .map_err(|e| GraphError::LoadFailure(format!("Failed to load graph {e:?}")))?;
    Ok(())
}

pub fn load_edge_props_from_pandas(
    graph: &GraphStorage,
    df: &PyAny,
    src: &str,
    dst: &str,
    constant_properties: Option<&[&str]>,
    shared_constant_properties: Option<&HashMap<String, Prop>>,
    layer: Option<&str>,
    layer_col: Option<&str>,
) -> Result<(), GraphError> {
    Python::with_gil(|py| {
        let mut cols_to_check = vec![src, dst];
        if let Some(ref layer_col) = layer_col {
            cols_to_check.push(layer_col.as_ref());
        }
        cols_to_check.extend(constant_properties.unwrap_or(&Vec::new()));
        let df_view = process_pandas_py_df(df, py, cols_to_check.clone())?;
        df_view.check_cols_exist(&cols_to_check)?;
        load_edges_props_from_df(
            df_view,
            src,
            dst,
            constant_properties,
            shared_constant_properties,
            layer,
            layer_col,
            graph,
        )
        .map_err(|e| GraphLoadException::new_err(format!("{:?}", e)))?;
        Ok::<(), PyErr>(())
    })
    .map_err(|e| GraphError::LoadFailure(format!("Failed to load graph {e:?}")))?;
    Ok(())
}

pub fn load_edge_deletions_from_pandas(
    graph: &GraphStorage,
    df: &PyAny,
    time: &str,
    src: &str,
    dst: &str,
    layer: Option<&str>,
    layer_col: Option<&str>,
) -> Result<(), GraphError> {
    Python::with_gil(|py| {
        let mut cols_to_check = vec![src, dst, time];
        if let Some(ref layer_col) = layer_col {
            cols_to_check.push(layer_col.as_ref());
        }

        let df_view = process_pandas_py_df(df, py, cols_to_check.clone())?;
        df_view.check_cols_exist(&cols_to_check)?;
        load_edge_deletions_from_df(
            df_view,
            time,
            src,
            dst,
            layer,
            layer_col,
            graph.core_graph(),
        )
        .map_err(|e| GraphLoadException::new_err(format!("{:?}", e)))?;
        Ok::<(), PyErr>(())
    })
    .map_err(|e| GraphError::LoadFailure(format!("Failed to load graph {e:?}")))?;
    Ok(())
}

pub(crate) fn process_pandas_py_df<'a>(
    df: &'a PyAny,
    py: Python<'a>,
    col_names: Vec<&str>,
) -> PyResult<DFView<impl Iterator<Item = Result<DFChunk, GraphError>> + 'a>> {
    is_jupyter(py);
    py.import("pandas")?;
    let module = py.import("pyarrow")?;
    let pa_table = module.getattr("Table")?;

    let df_columns: Vec<String> = df.getattr("columns")?.extract()?;

    let cols_to_drop: Vec<String> = df_columns
        .into_iter()
        .filter(|x| !col_names.contains(&x.as_str()))
        .collect();

    let dropped_df = if !cols_to_drop.is_empty() {
        let drop_method = df.getattr("drop")?;
        drop_method.call((cols_to_drop,), Some(vec![("axis", 1)].into_py_dict(py)))?
    } else {
        df
    };

    let _df_columns: Vec<String> = dropped_df.getattr("columns")?.extract()?;

    let table = pa_table.call_method("from_pandas", (dropped_df,), None)?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("max_chunksize", 100000)?;
    let rb = table
        .call_method("to_batches", (), Some(kwargs))?
        .extract::<Vec<&PyAny>>()?;
    let names: Vec<String> = if let Some(batch0) = rb.get(0) {
        let schema = batch0.getattr("schema")?;
        schema.getattr("names")?.extract::<Vec<String>>()?
    } else {
        vec![]
    }
    .into_iter()
    .filter(|x| col_names.contains(&x.as_str()))
    .collect();

    let names_len = names.len();
    let chunks = rb.into_iter().map(move |rb| {
        let chunk = (0..names_len)
            .map(|i| {
                let array = rb
                    .call_method1("column", (i,))
                    .map_err(|e| GraphError::from(e))?;
                let arr = array_to_rust(array).map_err(|e| GraphError::from(e))?;
                Ok::<Box<dyn Array>, GraphError>(arr)
            })
            .collect::<Result<Vec<_>, GraphError>>()?;

        Ok(DFChunk { chunk })
    });
    let num_rows: usize = py
        .eval(
            "index.__len__()",
            Some([("index", df.getattr("index")?)].into_py_dict(py)),
            None,
        )?
        .extract()?;

    Ok(DFView {
        names,
        chunks,
        num_rows,
    })
}

pub fn array_to_rust(obj: &PyAny) -> PyResult<ArrayRef> {
    // prepare a pointer to receive the Array struct
    let array = Box::new(ffi::ArrowArray::empty());
    let schema = Box::new(ffi::ArrowSchema::empty());

    let array_ptr = &*array as *const ffi::ArrowArray;
    let schema_ptr = &*schema as *const ffi::ArrowSchema;

    // make the conversion through PyArrow's private API
    // this changes the pointer's memory and is thus unsafe. In particular, `_export_to_c` can go out of bounds
    obj.call_method1(
        "_export_to_c",
        (array_ptr as Py_uintptr_t, schema_ptr as Py_uintptr_t),
    )?;

    unsafe {
        let field = ffi::import_field_from_c(schema.as_ref())
            .map_err(|e| ArrowErrorException::new_err(format!("{:?}", e)))?;

        let array = ffi::import_array_from_c(*array, field.data_type)
            .map_err(|e| ArrowErrorException::new_err(format!("{:?}", e)))?;

        Ok(array)
    }
}

fn is_jupyter(py: Python) {
    let code = r#"
try:
    shell = get_ipython().__class__.__name__
    if shell == 'ZMQInteractiveShell':
        result = True   # Jupyter notebook or qtconsole
    elif shell == 'TerminalInteractiveShell':
        result = False  # Terminal running IPython
    else:
        result = False  # Other type, assuming not a Jupyter environment
except NameError:
    result = False      # Probably standard Python interpreter
"#;

    if let Err(e) = py.run(code, None, None) {
        println!("Error checking if running in a jupyter notebook: {}", e);
        return;
    }

    match py.eval("result", None, None) {
        Ok(x) => {
            if let Ok(x) = x.extract() {
                kdam::set_notebook(x);
            }
        }
        Err(e) => {
            println!("Error checking if running in a jupyter notebook: {}", e);
        }
    };
}