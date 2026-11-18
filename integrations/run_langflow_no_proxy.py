"""Start the dedicated local SIC UI without inherited proxy settings."""
import os
import runpy
import sys

for key in ("http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "no_proxy", "all_proxy"):
    os.environ.pop(key, None)
sys.argv = ["langflow", *sys.argv[1:]]
runpy.run_module("langflow", run_name="__main__")
