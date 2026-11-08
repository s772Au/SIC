"""Official Studio domain graph for real per-context, per-level WB/CB work."""
from sic_workflow.domain_graphs import make_graph
from sic_workflow.hierarchy_forms import HierarchyForm, hierarchy_request, hierarchy_rows

graph = make_graph('wb_cb_hierarchy_and_replay', HierarchyForm, 'hierarchy.get', hierarchy_request, presenter=hierarchy_rows)
