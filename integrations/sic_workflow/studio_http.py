"""Official Agent Server http.app: standard, narrowly scoped Studio CORS.

langgraph-api 0.17.1 filters allow_private_network from http.cors. Its supported
custom-app middleware extension accepts the standard Starlette CORS option.
No routes/UI, proxies, request-header rewriting or auth replacements live here.
"""
from starlette.applications import Starlette
from starlette.middleware import Middleware
from starlette.middleware.cors import CORSMiddleware

app = Starlette(middleware=[Middleware(
    CORSMiddleware,
    allow_origins=['https://smith.langchain.com'],
    allow_methods=['*'],
    allow_headers=['*'],
    allow_credentials=True,
    allow_private_network=True,
    expose_headers=['x-pagination-total', 'x-pagination-next', 'content-location'],
)])
