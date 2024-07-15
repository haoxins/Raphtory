###############################################################################
#                                                                             #
#                      AUTOGENERATED TYPE STUB FILE                           #
#                                                                             #
#    This file was automatically generated. Do not modify it directly.        #
#    Any changes made here may be lost when the file is regenerated.          #
#                                                                             #
###############################################################################

class GraphqlGraphs:
    """
    A class for accessing graphs hosted in a Raphtory GraphQL server and running global search for
    graph documents
    """

    def __init__(self):
        """Initialize self.  See help(type(self)) for accurate signature."""

    def get(self, name):
        """Return the `VectorisedGraph` with name `name` or `None` if it doesn't exist"""

    def search_graph_documents(self, query, limit, window):
        """
        Return the top documents with the smallest cosine distance to `query`

        # Arguments
          * query - the text or the embedding to score against
          * limit - the maximum number of documents to return
          * window - the window where documents need to belong to in order to be considered

        # Returns
          A list of documents
        """

    def search_graph_documents_with_scores(self, query, limit, window):
        """Same as `search_graph_documents` but it also returns the scores alongside the documents"""

class RaphtoryClient:
    """A client for handling GraphQL operations in the context of Raphtory."""

    def __init__(self, url):
        """Initialize self.  See help(type(self)) for accurate signature."""

    def load_graphs_from_path(self, path, overwrite=False):
        """
        Set the server to load all the graphs from its path `path`.

        Arguments:
          * `path`: the path to load the graphs from.
          * `overwrite`: whether or not to overwrite existing graphs (defaults to False)

        Returns:
           The `data` field from the graphQL response after executing the mutation.
        """

    def query(self, query, variables=None):
        """
        Make a graphQL query against the server.

        Arguments:
          * `query`: the query to make.
          * `variables`: a dict of variables present on the query and their values.

        Returns:
           The `data` field from the graphQL response.
        """

    def send_graph(self, name, graph):
        """
        Send a graph to the server.

        Arguments:
          * `name`: the name of the graph sent.
          * `graph`: the graph to send.

        Returns:
           The `data` field from the graphQL response after executing the mutation.
        """

    def wait_for_online(self, millis=None):
        """
        Wait for the server to be online.

        Arguments:
          * `millis`: the minimum number of milliseconds to wait (default 5000).
        """

class RaphtoryServer:
    """A class for defining and running a Raphtory GraphQL server"""

    def __init__(self, graphs=None, graph_dir=None):
        """Initialize self.  See help(type(self)) for accurate signature."""

    def run(self, port=1736, log_level=..., enable_tracing=False, enable_auth=False):
        """
        Run the server until completion.

        Arguments:
          * `port`: the port to use (defaults to 1736).
        """

    def start(self, port=1736, log_level=..., enable_tracing=False, enable_auth=False):
        """
        Start the server and return a handle to it.

        Arguments:
          * `port`: the port to use (defaults to 1736).
        """

    def with_document_search_function(self, name, input, function):
        """
        Register a function in the GraphQL schema for document search over a graph.

        The function needs to take a `VectorisedGraph` as the first argument followed by a
        pre-defined set of keyword arguments. Supported types are `str`, `int`, and `float`.
        They have to be specified using the `input` parameter as a dict where the keys are the
        names of the parameters and the values are the types, expressed as strings.

        Arguments:
          * `name` (`str`): the name of the function in the GraphQL schema.
          * `input` (`dict`): the keyword arguments expected by the function.
          * `function` (`function`): the function to run.

        Returns:
           A new server object containing the vectorised graphs.
        """

    def with_global_search_function(self, name, input, function):
        """
        Register a function in the GraphQL schema for document search among all the graphs.

        The function needs to take a `GraphqlGraphs` object as the first argument followed by a
        pre-defined set of keyword arguments. Supported types are `str`, `int`, and `float`.
        They have to be specified using the `input` parameter as a dict where the keys are the
        names of the parameters and the values are the types, expressed as strings.

        Arguments:
          * `name` (`str`): the name of the function in the GraphQL schema.
          * `input` (`dict`): the keyword arguments expected by the function.
          * `function` (`function`): the function to run.

        Returns:
           A new server object containing the vectorised graphs.
        """

    def with_vectorised(self, cache, graph_names=None, embedding=None, graph_document=None, node_document=None, edge_document=None):
        """
        Vectorise a subset of the graphs of the server.

        Note:
          If no embedding function is provided, the server will attempt to use the OpenAI API
          embedding model, which will only work if the env variable OPENAI_API_KEY is set
          appropriately

        Arguments:
          * `graph_names`: the names of the graphs to vectorise. All by default.
          * `cache`: the directory to use as cache for the embeddings.
          * `embedding`: the embedding function to translate documents to embeddings.
          * `node_document`: the property name to use as the source for the documents on nodes.
          * `edge_document`: the property name to use as the source for the documents on edges.

        Returns:
           A new server object containing the vectorised graphs.
        """

class RunningRaphtoryServer:
    """A Raphtory server handler that also enables querying the server"""

    def __init__(self):
        """Initialize self.  See help(type(self)) for accurate signature."""

    def load_graphs_from_path(self, path, overwrite=False):
        """
        Set the server to load all the graphs from its path `path`.

        Arguments:
          * `path`: the path to load the graphs from.
          * `overwrite`: whether or not to overwrite existing graphs (defaults to False)

        Returns:
           The `data` field from the graphQL response after executing the mutation.
        """

    def query(self, query, variables=None):
        """
        Make a graphQL query against the server.

        Arguments:
          * `query`: the query to make.
          * `variables`: a dict of variables present on the query and their values.

        Returns:
           The `data` field from the graphQL response.
        """

    def send_graph(self, name, graph):
        """
        Send a graph to the server.

        Arguments:
          * `name`: the name of the graph sent.
          * `graph`: the graph to send.

        Returns:
           The `data` field from the graphQL response after executing the mutation.
        """

    def stop(self):
        """Stop the server."""

    def wait(self):
        """Wait until server completion."""

    def wait_for_online(self, timeout_millis=None):
        """
        Wait for the server to be online.

        Arguments:
          * `timeout_millis`: the timeout in milliseconds (default 5000).
        """
