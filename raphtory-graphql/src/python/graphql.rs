use crate::{
    model::algorithms::{
        algorithm_entry_point::AlgorithmEntryPoint, document::GqlDocument,
        global_plugins::GlobalPlugins, vector_algorithms::VectorAlgorithms,
    },
    server_config::*,
    url_encode::{url_decode_graph, url_encode_graph},
    GraphServer,
};
use async_graphql::{
    dynamic::{Field, FieldFuture, FieldValue, InputValue, Object, TypeRef, ValueAccessor},
    Value as GraphqlValue,
};
use crossbeam_channel::Sender as CrossbeamSender;
use dynamic_graphql::internal::{Registry, TypeName};
use itertools::intersperse;
use pyo3::{
    exceptions,
    exceptions::{PyAttributeError, PyException, PyTypeError, PyValueError},
    prelude::*,
    types::{IntoPyDict, PyDict, PyFunction, PyList},
};
use raphtory::{
    db::api::view::MaterializedGraph,
    python::{
        packages::vectors::{
            compute_embedding, into_py_document, translate_py_window, PyDocumentTemplate, PyQuery,
            PyVectorisedGraph, PyWindow,
        },
        types::wrappers::document::PyDocument,
        utils::{errors::adapt_err_value, execute_async_task},
    },
    vectors::{
        embeddings::openai_embedding, vectorised_cluster::VectorisedCluster, Document,
        EmbeddingFunction,
    },
};
use reqwest::{multipart, multipart::Part, Client};
use serde_json::{json, Map, Number, Value as JsonValue};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    thread,
    thread::{sleep, JoinHandle},
    time::Duration,
};
use tokio::{self, io::Result as IoResult, runtime::Runtime};

/// A class for accessing graphs hosted in a Raphtory GraphQL server and running global search for
/// graph documents
#[pyclass(name = "GraphqlGraphs")]
pub struct PyGlobalPlugins(GlobalPlugins);

#[pymethods]
impl PyGlobalPlugins {
    /// Return the top documents with the smallest cosine distance to `query`
    ///
    /// # Arguments
    ///   * query - the text or the embedding to score against
    ///   * limit - the maximum number of documents to return
    ///   * window - the window where documents need to belong to in order to be considered
    ///
    /// # Returns
    ///   A list of documents
    fn search_graph_documents(
        &self,
        py: Python,
        query: PyQuery,
        limit: usize,
        window: PyWindow,
    ) -> Vec<PyDocument> {
        self.search_graph_documents_with_scores(py, query, limit, window)
            .into_iter()
            .map(|(doc, _score)| doc)
            .collect()
    }

    /// Same as `search_graph_documents` but it also returns the scores alongside the documents
    fn search_graph_documents_with_scores(
        &self,
        py: Python,
        query: PyQuery,
        limit: usize,
        window: PyWindow,
    ) -> Vec<(PyDocument, f32)> {
        let window = translate_py_window(window);
        let graphs = self.0.vectorised_graphs.read();
        let cluster = VectorisedCluster::new(&graphs);
        let vectorised_graphs = self.0.vectorised_graphs.read();
        let graph_entry = vectorised_graphs.iter().next();
        let (_, first_graph) = graph_entry
            .expect("trying to search documents with no vectorised graphs on the server");
        let embedding = compute_embedding(first_graph, query);
        let documents = cluster.search_graph_documents_with_scores(&embedding, limit, window);
        documents.into_iter().map(|(doc, score)| {
            let graph = match &doc {
                Document::Graph { name, .. } => {
                    vectorised_graphs.get(name).unwrap()
                }
                _ => panic!("search_graph_documents_with_scores returned a document that is not from a graph"),
            };
            (into_py_document(doc, graph, py), score)
        }).collect()
    }

    /// Return the `VectorisedGraph` with name `name` or `None` if it doesn't exist
    fn get(&self, name: &str) -> Option<PyVectorisedGraph> {
        self.0
            .vectorised_graphs
            .read()
            .get(name)
            .map(|graph| graph.clone().into())
    }
}

/// A class for defining and running a Raphtory GraphQL server
#[pyclass(name = "GraphServer")]
pub struct PyGraphServer(Option<GraphServer>);

impl PyGraphServer {
    fn new(server: GraphServer) -> Self {
        Self(Some(server))
    }

    fn with_vectorised_generic_embedding<F: EmbeddingFunction + Clone + 'static>(
        slf: PyRefMut<Self>,
        graph_names: Option<Vec<String>>,
        embedding: F,
        cache: String,
        graph_document: Option<String>,
        node_document: Option<String>,
        edge_document: Option<String>,
    ) -> PyResult<Self> {
        let template = PyDocumentTemplate::new(graph_document, node_document, edge_document);
        let server = take_server_ownership(slf)?;
        execute_async_task(move || async move {
            let new_server = server
                .with_vectorised(
                    graph_names,
                    embedding,
                    &PathBuf::from(cache),
                    Some(template),
                )
                .await?;
            Ok(Self::new(new_server))
        })
    }

    fn with_generic_document_search_function<
        'a,
        E: AlgorithmEntryPoint<'a> + 'static,
        F: Fn(&E, Python) -> PyObject + Send + Sync + 'static,
    >(
        slf: PyRefMut<Self>,
        name: String,
        input: HashMap<String, String>,
        function: &PyFunction,
        adapter: F,
    ) -> PyResult<Self> {
        let function: Py<PyFunction> = function.into();

        let input_mapper = HashMap::from([
            ("str", TypeRef::named_nn(TypeRef::STRING)),
            ("int", TypeRef::named_nn(TypeRef::INT)),
            ("float", TypeRef::named_nn(TypeRef::FLOAT)),
        ]);

        let input_values = input
            .into_iter()
            .map(|(name, type_name)| {
                let type_ref = input_mapper.get(&type_name.as_str()).cloned();
                type_ref
                    .map(|type_ref| InputValue::new(name, type_ref))
                    .ok_or_else(|| {
                        let valid_types = input_mapper.keys().map(|key| key.to_owned());
                        let valid_types_string: String = intersperse(valid_types, ", ").collect();
                        let msg = format!("types in input have to be one of: {valid_types_string}");
                        PyAttributeError::new_err(msg)
                    })
            })
            .collect::<PyResult<Vec<InputValue>>>()?;

        let register_function = |name: &str, registry: Registry, parent: Object| {
            let registry = registry.register::<GqlDocument>();
            let output_type = TypeRef::named_nn_list_nn(GqlDocument::get_type_name());
            let mut field = Field::new(name, output_type, move |ctx| {
                let documents = Python::with_gil(|py| {
                    let entry_point = adapter(ctx.parent_value.downcast_ref().unwrap(), py);
                    let kw_args: HashMap<&str, PyObject> = ctx
                        .args
                        .iter()
                        .map(|(name, value)| (name.as_str(), adapt_graphql_value(&value, py)))
                        .collect();
                    let py_kw_args = kw_args.into_py_dict(py);
                    let result = function.call(py, (entry_point,), Some(py_kw_args)).unwrap();
                    let list = result.downcast::<PyList>(py).unwrap();
                    let py_documents = list.iter().map(|doc| doc.extract::<PyDocument>().unwrap());
                    py_documents
                        .map(|doc| doc.extract_rust_document(py).unwrap())
                        .collect::<Vec<_>>()
                });

                let gql_documents = documents
                    .into_iter()
                    .map(|doc| FieldValue::owned_any(GqlDocument::from(doc)));

                FieldFuture::Value(Some(FieldValue::list(gql_documents)))
            });
            for input_value in input_values {
                field = field.argument(input_value);
            }
            let parent = parent.field(field);
            (registry, parent)
        };
        E::lock_plugins().insert(name, Box::new(register_function));

        let new_server = take_server_ownership(slf)?;
        Ok(Self::new(new_server))
    }
}

#[pymethods]
impl PyGraphServer {
    #[new]
    #[pyo3(
        signature = (work_dir, cache_capacity = None, cache_tti_seconds = None, log_level = None, config_path = None)
    )]
    fn py_new(
        work_dir: PathBuf,
        cache_capacity: Option<u64>,
        cache_tti_seconds: Option<u64>,
        log_level: Option<String>,
        config_path: Option<PathBuf>,
    ) -> PyResult<Self> {
        let mut app_config_builder = AppConfigBuilder::new();
        if let Some(log_level) = log_level {
            app_config_builder = app_config_builder.with_log_level(log_level);
        }
        if let Some(cache_capacity) = cache_capacity {
            app_config_builder = app_config_builder.with_cache_capacity(cache_capacity);
        }
        if let Some(cache_tti_seconds) = cache_tti_seconds {
            app_config_builder = app_config_builder.with_cache_tti_seconds(cache_tti_seconds);
        }
        let app_config = Some(app_config_builder.build());

        let server = GraphServer::new(work_dir, app_config, config_path)?;
        Ok(PyGraphServer::new(server))
    }

    /// Vectorise a subset of the graphs of the server.
    ///
    /// Note:
    ///   If no embedding function is provided, the server will attempt to use the OpenAI API
    ///   embedding model, which will only work if the env variable OPENAI_API_KEY is set
    ///   appropriately
    ///
    /// Arguments:
    ///   * `graph_names`: the names of the graphs to vectorise. All by default.
    ///   * `cache`: the directory to use as cache for the embeddings.
    ///   * `embedding`: the embedding function to translate documents to embeddings.
    ///   * `node_document`: the property name to use as the source for the documents on nodes.
    ///   * `edge_document`: the property name to use as the source for the documents on edges.
    ///
    /// Returns:
    ///    A new server object containing the vectorised graphs.
    fn with_vectorised(
        slf: PyRefMut<Self>,
        cache: String,
        graph_names: Option<Vec<String>>,
        // TODO: support more models by just providing a string, e.g. "openai", here and in the VectorisedGraph API
        embedding: Option<&PyFunction>,
        graph_document: Option<String>,
        node_document: Option<String>,
        edge_document: Option<String>,
    ) -> PyResult<Self> {
        match embedding {
            Some(embedding) => {
                let embedding: Py<PyFunction> = embedding.into();
                Self::with_vectorised_generic_embedding(
                    slf,
                    graph_names,
                    embedding,
                    cache,
                    graph_document,
                    node_document,
                    edge_document,
                )
            }
            None => Self::with_vectorised_generic_embedding(
                slf,
                graph_names,
                openai_embedding,
                cache,
                graph_document,
                node_document,
                edge_document,
            ),
        }
    }

    /// Register a function in the GraphQL schema for document search over a graph.
    ///
    /// The function needs to take a `VectorisedGraph` as the first argument followed by a
    /// pre-defined set of keyword arguments. Supported types are `str`, `int`, and `float`.
    /// They have to be specified using the `input` parameter as a dict where the keys are the
    /// names of the parameters and the values are the types, expressed as strings.
    ///
    /// Arguments:
    ///   * `name` (`str`): the name of the function in the GraphQL schema.
    ///   * `input` (`dict`): the keyword arguments expected by the function.
    ///   * `function` (`function`): the function to run.
    ///
    /// Returns:
    ///    A new server object containing the vectorised graphs.
    pub fn with_document_search_function(
        slf: PyRefMut<Self>,
        name: String,
        input: HashMap<String, String>,
        function: &PyFunction,
    ) -> PyResult<Self> {
        let adapter =
            |entry_point: &VectorAlgorithms, py: Python| entry_point.graph.clone().into_py(py);
        PyGraphServer::with_generic_document_search_function(slf, name, input, function, adapter)
    }

    /// Register a function in the GraphQL schema for document search among all the graphs.
    ///
    /// The function needs to take a `GraphqlGraphs` object as the first argument followed by a
    /// pre-defined set of keyword arguments. Supported types are `str`, `int`, and `float`.
    /// They have to be specified using the `input` parameter as a dict where the keys are the
    /// names of the parameters and the values are the types, expressed as strings.
    ///
    /// Arguments:
    ///   * `name` (`str`): the name of the function in the GraphQL schema.
    ///   * `input` (`dict`): the keyword arguments expected by the function.
    ///   * `function` (`function`): the function to run.
    ///
    /// Returns:
    ///    A new server object containing the vectorised graphs.
    pub fn with_global_search_function(
        slf: PyRefMut<Self>,
        name: String,
        input: HashMap<String, String>,
        function: &PyFunction,
    ) -> PyResult<Self> {
        let adapter = |entry_point: &GlobalPlugins, py: Python| {
            PyGlobalPlugins(entry_point.clone()).into_py(py)
        };
        PyGraphServer::with_generic_document_search_function(slf, name, input, function, adapter)
    }

    /// Start the server and return a handle to it.
    ///
    /// Arguments:
    ///   * `port`: the port to use (defaults to 1736).
    ///   * `timeout_ms`: wait for server to be online (defaults to 5000). The server is stopped if not online within timeout_ms but manages to come online as soon as timeout_ms finishes!
    #[pyo3(
        signature = (port = 1736, timeout_ms = None)
    )]
    pub fn start(
        slf: PyRefMut<Self>,
        py: Python,
        port: u16,
        timeout_ms: Option<u64>,
    ) -> PyResult<PyRunningGraphServer> {
        let (sender, receiver) = crossbeam_channel::bounded::<BridgeCommand>(1);
        let server = take_server_ownership(slf)?;

        let cloned_sender = sender.clone();

        let join_handle = thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let handler = server.start_with_port(port);
                    let running_server = handler.await?;
                    let tokio_sender = running_server._get_sender().clone();
                    tokio::task::spawn_blocking(move || {
                        match receiver.recv().expect("Failed to wait for cancellation") {
                            BridgeCommand::StopServer => tokio_sender
                                .blocking_send(())
                                .expect("Failed to send cancellation signal"),
                            BridgeCommand::StopListening => (),
                        }
                    });
                    let result = running_server.wait().await;
                    _ = cloned_sender.send(BridgeCommand::StopListening);
                    result
                })
        });

        let mut server = PyRunningGraphServer::new(join_handle, sender, port)?;
        if let Some(server_handler) = &server.server_handler {
            match PyRunningGraphServer::wait_for_server_online(
                &server_handler.client.url,
                timeout_ms,
            ) {
                Ok(_) => return Ok(server),
                Err(e) => {
                    PyRunningGraphServer::stop_server(&mut server, py)?;
                    Err(e)
                }
            }
        } else {
            Err(PyException::new_err("Failed to start server"))
        }
    }

    /// Run the server until completion.
    ///
    /// Arguments:
    ///   * `port`: the port to use (defaults to 1736).
    #[pyo3(
        signature = (port = 1736, timeout_ms = Some(180000))
    )]
    pub fn run(
        slf: PyRefMut<Self>,
        py: Python,
        port: u16,
        timeout_ms: Option<u64>,
    ) -> PyResult<()> {
        let mut server = Self::start(slf, py, port, timeout_ms)?.server_handler;
        py.allow_threads(|| wait_server(&mut server))
    }
}

fn adapt_graphql_value(value: &ValueAccessor, py: Python) -> PyObject {
    match value.as_value() {
        GraphqlValue::Number(number) => {
            if number.is_f64() {
                number.as_f64().unwrap().to_object(py)
            } else if number.is_u64() {
                number.as_u64().unwrap().to_object(py)
            } else {
                number.as_i64().unwrap().to_object(py)
            }
        }
        GraphqlValue::String(value) => value.to_object(py),
        GraphqlValue::Boolean(value) => value.to_object(py),
        value => panic!("graphql input value {value} has an unsupported type"),
    }
}

fn take_server_ownership(mut server: PyRefMut<PyGraphServer>) -> PyResult<GraphServer> {
    let new_server = server.0.take().ok_or_else(|| {
        PyException::new_err(
            "Server object has already been used, please create another one from scratch",
        )
    })?;
    Ok(new_server)
}

fn wait_server(running_server: &mut Option<ServerHandler>) -> PyResult<()> {
    let owned_running_server = running_server
        .take()
        .ok_or_else(|| PyException::new_err(RUNNING_SERVER_CONSUMED_MSG))?;
    owned_running_server
        .join_handle
        .join()
        .expect("error when waiting for the server thread to complete")
        .map_err(|e| adapt_err_value(&e))
}

const RUNNING_SERVER_CONSUMED_MSG: &str =
    "Running server object has already been used, please create another one from scratch";

/// A Raphtory server handler that also enables querying the server
#[pyclass(name = "RunningGraphServer")]
pub struct PyRunningGraphServer {
    server_handler: Option<ServerHandler>,
}

enum BridgeCommand {
    StopServer,
    StopListening,
}

struct ServerHandler {
    join_handle: JoinHandle<IoResult<()>>,
    sender: CrossbeamSender<BridgeCommand>,
    client: PyRaphtoryClient,
}

impl PyRunningGraphServer {
    fn new(
        join_handle: JoinHandle<IoResult<()>>,
        sender: CrossbeamSender<BridgeCommand>,
        port: u16,
    ) -> PyResult<Self> {
        let url = format!("http://localhost:{port}");
        let server_handler = Some(ServerHandler {
            join_handle,
            sender,
            client: PyRaphtoryClient::new(url)?,
        });

        Ok(PyRunningGraphServer { server_handler })
    }

    fn apply_if_alive<O, F>(&self, function: F) -> PyResult<O>
    where
        F: FnOnce(&ServerHandler) -> PyResult<O>,
    {
        match &self.server_handler {
            Some(handler) => function(handler),
            None => Err(PyException::new_err(RUNNING_SERVER_CONSUMED_MSG)),
        }
    }

    fn wait_for_server_online(url: &String, timeout_ms: Option<u64>) -> PyResult<()> {
        let millis = timeout_ms.unwrap_or(5000);
        let num_intervals = millis / WAIT_CHECK_INTERVAL_MILLIS;

        for _ in 0..num_intervals {
            if is_online(url) {
                return Ok(());
            } else {
                sleep(Duration::from_millis(WAIT_CHECK_INTERVAL_MILLIS))
            }
        }

        Err(PyException::new_err(format!(
            "Failed to start server in {} milliseconds",
            millis
        )))
    }

    fn stop_server(&mut self, py: Python) -> PyResult<()> {
        Self::apply_if_alive(self, |handler| {
            handler
                .sender
                .send(BridgeCommand::StopServer)
                .expect("Failed when sending cancellation signal");
            Ok(())
        })?;
        let server = &mut self.server_handler;
        py.allow_threads(|| wait_server(server))
    }
}

#[pymethods]
impl PyRunningGraphServer {
    pub(crate) fn get_client(&self) -> PyResult<PyRaphtoryClient> {
        self.apply_if_alive(|handler| Ok(handler.client.clone()))
    }

    /// Stop the server and wait for it to finish
    pub(crate) fn stop(&mut self, py: Python) -> PyResult<()> {
        self.stop_server(py)
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(
        &mut self,
        py: Python,
        _exc_type: PyObject,
        _exc_val: PyObject,
        _exc_tb: PyObject,
    ) -> PyResult<()> {
        self.stop_server(py)
    }
}

fn is_online(url: &String) -> bool {
    reqwest::blocking::get(url)
        .map(|response| response.status().as_u16() == 200)
        .unwrap_or(false)
}

/// A client for handling GraphQL operations in the context of Raphtory.
#[derive(Clone)]
#[pyclass(name = "RaphtoryClient")]
pub struct PyRaphtoryClient {
    pub(crate) url: String,
}

impl PyRaphtoryClient {
    fn query_with_json_variables(
        &self,
        query: String,
        variables: HashMap<String, JsonValue>,
    ) -> PyResult<HashMap<String, JsonValue>> {
        let client = self.clone();
        let (graphql_query, graphql_result) = execute_async_task(move || async move {
            client.send_graphql_query(query, variables).await
        })?;
        let mut graphql_result = graphql_result;
        match graphql_result.remove("data") {
            Some(JsonValue::Object(data)) => Ok(data.into_iter().collect()),
            _ => match graphql_result.remove("errors") {
                Some(JsonValue::Array(errors)) => Err(PyException::new_err(format!(
                    "After sending query to the server:\n\t{graphql_query}\nGot the following errors:\n\t{errors:?}"
                ))),
                _ => Err(PyException::new_err(format!(
                    "Error while reading server response for query:\n\t{graphql_query}"
                ))),
            },
        }
    }

    /// Returns the query sent and the response
    async fn send_graphql_query(
        &self,
        query: String,
        variables: HashMap<String, JsonValue>,
    ) -> PyResult<(JsonValue, HashMap<String, JsonValue>)> {
        let client = Client::new();

        let request_body = json!({
            "query": query,
            "variables": variables
        });

        let response = client
            .post(&self.url)
            .json(&request_body)
            .send()
            .await
            .map_err(|err| adapt_err_value(&err))?;

        response
            .json()
            .await
            .map_err(|err| adapt_err_value(&err))
            .map(|json| (request_body, json))
    }
}

const WAIT_CHECK_INTERVAL_MILLIS: u64 = 200;

#[pymethods]
impl PyRaphtoryClient {
    #[new]
    fn new(url: String) -> PyResult<Self> {
        match reqwest::blocking::get(url.clone()) {
            Ok(response) => {
                if response.status() == 200 {
                    Ok(Self { url })
                } else {
                    Err(PyValueError::new_err(format!(
                        "Could not connect to the given server - response {}",
                        response.status()
                    )))
                }
            }
            Err(_) => Err(PyValueError::new_err(
                "Could not connect to the given server - no response",
            )),
        }
    }

    /// Check if the server is online.
    ///
    /// Returns:
    ///    Returns true if server is online otherwise false.
    fn is_server_online(&self) -> PyResult<bool> {
        Ok(is_online(&self.url))
    }

    /// Make a graphQL query against the server.
    ///
    /// Arguments:
    ///   * `query`: the query to make.
    ///   * `variables`: a dict of variables present on the query and their values.
    ///
    /// Returns:
    ///    The `data` field from the graphQL response.
    fn query(
        &self,
        py: Python,
        query: String,
        variables: Option<HashMap<String, PyObject>>,
    ) -> PyResult<HashMap<String, PyObject>> {
        let variables = variables.unwrap_or_else(|| HashMap::new());
        let mut json_variables = HashMap::new();
        for (key, value) in variables {
            let json_value = translate_from_python(py, value)?;
            json_variables.insert(key, json_value);
        }

        let data = self.query_with_json_variables(query, json_variables)?;
        translate_map_to_python(py, data)
    }

    /// Send a graph to the server
    ///
    /// Arguments:
    ///   * `path`: the path of the graph
    ///   * `graph`: the graph to send
    ///   * `overwrite`: overwrite existing graph (defaults to False)
    ///
    /// Returns:
    ///    The `data` field from the graphQL response after executing the mutation.
    #[pyo3(signature = (path, graph, overwrite = false))]
    fn send_graph(&self, path: String, graph: MaterializedGraph, overwrite: bool) -> PyResult<()> {
        let encoded_graph = encode_graph(graph)?;

        let query = r#"
            mutation SendGraph($path: String!, $graph: String!, $overwrite: Boolean!) {
                sendGraph(path: $path, graph: $graph, overwrite: $overwrite)
            }
        "#
        .to_owned();
        let variables = [
            ("path".to_owned(), json!(path)),
            ("graph".to_owned(), json!(encoded_graph)),
            ("overwrite".to_owned(), json!(overwrite)),
        ];

        let data = self.query_with_json_variables(query, variables.into())?;

        match data.get("sendGraph") {
            Some(JsonValue::String(name)) => {
                println!("Sent graph '{name}' to the server");
                Ok(())
            }
            _ => Err(PyException::new_err(format!(
                "Error Sending Graph. Got response {:?}",
                data
            ))),
        }
    }

    /// Upload graph file from a path `file_path` on the client
    ///
    /// Arguments:
    ///   * `path`: the name of the graph
    ///   * `file_path`: the path of the graph on the client
    ///   * `overwrite`: overwrite existing graph (defaults to False)
    ///
    /// Returns:
    ///    The `data` field from the graphQL response after executing the mutation.
    #[pyo3(signature = (path, file_path, overwrite = false))]
    fn upload_graph(&self, path: String, file_path: String, overwrite: bool) -> PyResult<()> {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let client = Client::new();

            let mut file = File::open(Path::new(&file_path)).map_err(|err| adapt_err_value(&err))?;

            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).map_err(|err| adapt_err_value(&err))?;

            let variables = format!(
                r#""path": "{}", "overwrite": {}, "graph": null"#,
                path, overwrite
            );

            let operations = format!(
                r#"{{
            "query": "mutation UploadGraph($path: String!, $graph: Upload!, $overwrite: Boolean!) {{ uploadGraph(path: $path, graph: $graph, overwrite: $overwrite) }}",
            "variables": {{ {} }}
        }}"#,
                variables
            );

            let form = multipart::Form::new()
                .text("operations", operations)
                .text("map", r#"{"0": ["variables.graph"]}"#)
                .part("0", Part::bytes(buffer).file_name(file_path.clone()));

            let response = client
                .post(&self.url)
                .multipart(form)
                .send()
                .await
                .map_err(|err| adapt_err_value(&err))?;

            let status = response.status();
            let text = response.text().await.map_err(|err| adapt_err_value(&err))?;

            if !status.is_success() {
                return Err(PyException::new_err(format!(
                    "Error Uploading Graph. Status: {}. Response: {}",
                    status, text
                )));
            }

            let mut data: HashMap<String, JsonValue> = serde_json::from_str(&text).map_err(|err| {
                PyException::new_err(format!(
                    "Failed to parse JSON response: {}. Response text: {}",
                    err, text
                ))
            })?;

            match data.remove("data") {
                Some(JsonValue::Object(_)) => {
                    Ok(())
                }
                _ => match data.remove("errors") {
                    Some(JsonValue::Array(errors)) => Err(PyException::new_err(format!(
                        "Error Uploading Graph. Got errors:\n\t{:#?}",
                        errors
                    ))),
                    _ => Err(PyException::new_err(format!(
                        "Error Uploading Graph. Unexpected response: {}",
                        text
                    ))),
                },
            }
        })
    }

    // /// Load graph from a path `path` on the server.
    // ///
    // /// Arguments:
    // ///   * `file_path`: the path to load the graph from.
    // ///   * `overwrite`: overwrite existing graph (defaults to False)
    // ///   * `namespace`: the namespace of the graph (defaults to None)
    // ///
    // /// Returns:
    // ///    The `data` field from the graphQL response after executing the mutation.
    // #[pyo3(signature = (file_path, overwrite = false, namespace = None))]
    // fn load_graph(
    //     &self,
    //     py: Python,
    //     file_path: String,
    //     overwrite: bool,
    //     namespace: Option<String>,
    // ) -> PyResult<HashMap<String, PyObject>> {
    //     let query = r#"
    //         mutation LoadGraph($pathOnServer: String!, $overwrite: Boolean!, $namespace: String) {
    //             loadGraphFromPath(pathOnServer: $pathOnServer, overwrite: $overwrite, namespace: $namespace)
    //         }
    //     "#
    //         .to_owned();
    //     let variables = [
    //         ("pathOnServer".to_owned(), json!(file_path)),
    //         ("overwrite".to_owned(), json!(overwrite)),
    //         ("namespace".to_owned(), json!(namespace)),
    //     ];
    //
    //     let data = self.query_with_json_variables(query.clone(), variables.into())?;
    //
    //     match data.get("loadGraphFromPath") {
    //         Some(JsonValue::String(name)) => {
    //             println!("Loaded graph: '{name}'");
    //             translate_map_to_python(py, data)
    //         }
    //         _ => Err(PyException::new_err(format!(
    //             "Error while reading server response for query:\n\t{query}\nGot data:\n\t'{data:?}'"
    //         ))),
    //     }
    // }

    /// Copy graph from a path `path` on the server to a `new_path` on the server
    ///
    /// Arguments:
    ///   * `path`: the path of the graph to be copied
    ///   * `new_path`: the new path of the copied graph
    ///
    /// Returns:
    ///    Copy status as boolean
    #[pyo3(signature = (path, new_path))]
    fn copy_graph(&self, path: String, new_path: String) -> PyResult<()> {
        let query = r#"
            mutation CopyGraph($path: String!, $newPath: String!) {
              copyGraph(
                path: $path,
                newPath: $newPath,
              )
            }"#
        .to_owned();

        let variables = [
            ("path".to_owned(), json!(path)),
            ("newPath".to_owned(), json!(new_path)),
        ];

        let data = self.query_with_json_variables(query.clone(), variables.into())?;
        match data.get("copyGraph") {
            Some(JsonValue::Bool(res)) => Ok((*res).clone()),
            _ => Err(PyException::new_err(format!(
                "Error while reading server response for query:\n\t{query}\nGot data:\n\t'{data:?}'"
            ))),
        }?;
        Ok(())
    }

    /// Move graph from a path `path` on the server to a `new_path` on the server
    ///
    /// Arguments:
    ///   * `path`: the path of the graph to be moved
    ///   * `new_path`: the new path of the moved graph
    ///
    /// Returns:
    ///    Move status as boolean
    #[pyo3(signature = (path, new_path))]
    fn move_graph(&self, path: String, new_path: String) -> PyResult<()> {
        let query = r#"
            mutation MoveGraph($path: String!, $newPath: String!) {
              moveGraph(
                path: $path,
                newPath: $newPath,
              )
            }"#
        .to_owned();

        let variables = [
            ("path".to_owned(), json!(path)),
            ("newPath".to_owned(), json!(new_path)),
        ];

        let data = self.query_with_json_variables(query.clone(), variables.into())?;
        match data.get("moveGraph") {
            Some(JsonValue::Bool(res)) => Ok((*res).clone()),
            _ => Err(PyException::new_err(format!(
                "Error while reading server response for query:\n\t{query}\nGot data:\n\t'{data:?}'"
            ))),
        }?;
        Ok(())
    }

    /// Delete graph from a path `path` on the server
    ///
    /// Arguments:
    ///   * `path`: the path of the graph to be deleted
    ///
    /// Returns:
    ///    Delete status as boolean
    #[pyo3(signature = (path))]
    fn delete_graph(&self, path: String) -> PyResult<()> {
        let query = r#"
            mutation DeleteGraph($path: String!) {
              deleteGraph(
                path: $path,
              )
            }"#
        .to_owned();

        let variables = [("path".to_owned(), json!(path))];

        let data = self.query_with_json_variables(query.clone(), variables.into())?;
        match data.get("deleteGraph") {
            Some(JsonValue::Bool(res)) => Ok((*res).clone()),
            _ => Err(PyException::new_err(format!(
                "Error while reading server response for query:\n\t{query}\nGot data:\n\t'{data:?}'"
            ))),
        }?;
        Ok(())
    }

    /// Receive graph from a path `path` on the server
    ///
    /// Arguments:
    ///   * `path`: the path of the graph to be received
    ///
    /// Returns:
    ///    Graph as string
    fn receive_graph(&self, path: String) -> PyResult<MaterializedGraph> {
        let query = r#"
            query ReceiveGraph($path: String!) {
                receiveGraph(path: $path)
            }"#
        .to_owned();
        let variables = [("path".to_owned(), json!(path))];
        let data = self.query_with_json_variables(query.clone(), variables.into())?;
        match data.get("receiveGraph") {
            Some(JsonValue::String(graph)) => {
                let mat_graph = url_decode_graph(graph)?;
                Ok(mat_graph)
            }
            _ => Err(PyException::new_err(format!(
                "Error while reading server response for query:\n\t{query}\nGot data:\n\t'{data:?}'"
            ))),
        }
    }
}

fn translate_from_python(py: Python, value: PyObject) -> PyResult<JsonValue> {
    if let Ok(value) = value.extract::<i64>(py) {
        Ok(JsonValue::Number(value.into()))
    } else if let Ok(value) = value.extract::<f64>(py) {
        Ok(JsonValue::Number(Number::from_f64(value).unwrap()))
    } else if let Ok(value) = value.extract::<bool>(py) {
        Ok(JsonValue::Bool(value))
    } else if let Ok(value) = value.extract::<String>(py) {
        Ok(JsonValue::String(value))
    } else if let Ok(value) = value.extract::<Vec<PyObject>>(py) {
        let mut vec = Vec::new();
        for item in value {
            vec.push(translate_from_python(py, item)?);
        }
        Ok(JsonValue::Array(vec))
    } else if let Ok(value) = value.extract::<&PyDict>(py) {
        let mut map = Map::new();
        for (key, value) in value.iter() {
            let key = key.extract::<String>()?;
            let value = translate_from_python(py, value.into_py(py))?;
            map.insert(key, value);
        }
        Ok(JsonValue::Object(map))
    } else {
        Err(PyErr::new::<PyTypeError, _>("Unsupported type"))
    }
}

fn translate_map_to_python(
    py: Python,
    input: HashMap<String, JsonValue>,
) -> PyResult<HashMap<String, PyObject>> {
    let mut output_dict = HashMap::new();
    for (key, value) in input {
        let py_value = translate_to_python(py, value)?;
        output_dict.insert(key, py_value);
    }

    Ok(output_dict)
}

fn translate_to_python(py: Python, value: serde_json::Value) -> PyResult<PyObject> {
    match value {
        JsonValue::Number(num) => {
            if num.is_i64() {
                Ok(num.as_i64().unwrap().into_py(py))
            } else if num.is_f64() {
                Ok(num.as_f64().unwrap().into_py(py))
            } else {
                Err(PyErr::new::<PyTypeError, _>("Unsupported number type"))
            }
        }
        JsonValue::String(s) => Ok(s.into_py(py)),
        JsonValue::Array(vec) => {
            let mut list = Vec::new();
            for item in vec {
                list.push(translate_to_python(py, item)?);
            }
            Ok(list.into_py(py))
        }
        JsonValue::Object(map) => {
            let dict = PyDict::new(py);
            for (key, value) in map {
                dict.set_item(key, translate_to_python(py, value)?)?;
            }
            Ok(dict.into())
        }
        JsonValue::Bool(b) => Ok(b.into_py(py)),
        JsonValue::Null => Ok(py.None()),
    }
}

fn encode_graph(graph: MaterializedGraph) -> PyResult<String> {
    let result = url_encode_graph(graph);
    match result {
        Ok(s) => Ok(s),
        Err(e) => Err(PyValueError::new_err(format!("Error encoding: {:?}", e))),
    }
}