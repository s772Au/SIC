# Installable Python Core — public v0.1 prerelease

The Python distribution is `sic-core==0.1.0`; the import remains `sic_core`. This is the existing v0.1 release line. Individual builds are identified by source and artifact SHA256 hashes.

Python 3.12–3.14 is supported by the package metadata, matching the workflow integration. The base installation has no runtime dependencies: deterministic editing, WB/CB publication, hierarchy, P/V/I and local persistence do not require a model SDK, image library, GUI, Torch or corpus downloads.

## Install

Install a reviewed local wheel into a dedicated environment:

```powershell
python -m pip install <artifact-directory>/sic_core-0.1.0-py3-none-any.whl
```

The source distribution can also be installed with pip. Its only build requirement is the pinned setuptools backend. Optional features use the exact versions in the packaged `sic_core/requirements.txt`:

- `sic-core[provider]`: LiteLLM provider adapter.
- `sic-core[media]`: immutable image data-URI validation.
- `sic-core[full]`: both optional features.

These extras do not install Langflow, Studio or Torch. Native media still requires valid, known model-specific admission accounting; installing an extra does not create a model profile or authorize any request.

The package includes the operation catalog, ledger schema, empty configuration example and dependency lock. The `sic-core` command delegates to the existing authenticated HTTP server entry point; `sic-core --help` only displays its existing arguments. Importing `Core` or installing the package does not start a service.

## Embed without a server

```python
from sic_core import Core

core = Core()
context = {"actor": "example-host", "allow_write": True, "allow_model": False}
context["chain_id"] = core.operate("chain.create", context, {})["result"]["chain_id"]
# Invoke the existing Core.operate contract with explicit permissions.
core.close()
```

Credentials remain host environment variables. The configuration example contains environment variable names, not secret values. Dataset/media corpora, model weights, workflow packages and the JavaScript DSH plugin remain separate deliverables.

## Build and verify

The build script reads an explicit allowlist into an isolated staging directory. It writes only to a new output directory and its own temporary directories, and never publishes artifacts or manages running services.

```powershell
python scripts/build_python_core.py --phase provisional --output <new-output-directory>
```

Run with an environment containing the pinned setuptools build backend. The script builds wheel and sdist, audits their full member lists and Core file hashes, downloads that build backend as a separate build dependency, then installs **each artifact** into its own fresh environment outside the checkout. Those installs use no original-checkout PYTHONPATH and no optional model/image packages.

The installed self-check creates a Core, ingests source text, performs real program edits, publishes P/V/I, forms and edits real second- and third-order WBs, checks same-level WB/CB pairs and immutable history, and verifies packaged catalogs and the existing CLI help. It invokes no model and starts no service. These are mechanical installation checks, not research quality or cache-hit claims.

A final build must follow all Core contributor freezes. First inspect the complete release-input identity, then pass that exact identity:

```powershell
python scripts/build_python_core.py --inspect-source
python scripts/build_python_core.py --phase final --expected-source-sha256 <frozen-identity> --output <new-final-directory>
```

The script rejects changed inputs, unexpected package members, detected host paths or credentials, and source changes during the build. Only `sic_core` is a Python package. Tests, integrations, papers, video, local environments, legacy Rust, old experiment results, source data and weights are excluded. Original package code is MIT licensed; the preprint is separately CC BY 4.0. Third-party optional libraries retain their licenses. Build/installation commands do not themselves publish to PyPI, GitHub or Zenodo.


## Optional shared-prefix layout

`separate` remains the default. Configure `prefix_layout: shared_i` explicitly through the existing authorized `config.update` operation; a unified `shared_prefix_budget` and compatible logical model/provider/cache identity are required. The package contains the immutable plan, trusted mode/capability and exact local alias helpers. Incompatible scope, model budgets or media uses a complete, reported separate-layout fallback. Declared namespaces and equal SDK messages do not prove a physical provider cache hit.

New B workbench sources and isolated SDK/API paths passed their named checks; they have not yet been visually accepted on the standing official GUIs. Source-level evidence cannot validate an older installed wheel: consumers must compare the final wheel/sdist member hashes with the release source manifest. Historical installation checks apply to their recorded artifacts only.

The publication source build has 273 Core regression tests with zero failures/errors/skips; B-specific 115-test and actual SDK reports have their own source scope. These are distinct from the clean-install self-check and the 38 integration tests. Real DSH gateway runs remain small exploratory evidence, not a verified physical model, quality/cost advantage or cache result.

Late post-snapshot native/tool material can be retained by a program KEEP supplement; the record does not assert that a first-order Editor semantically processed that late content. See [current evidence and API/media limits](runtime-acceptance-current.md). Raw session logs, private manuscripts, videos, corpora and weights are not wheel/sdist contents. The separately published paper is [preprint v1](../paper/sic_paper_zh_v1.md); Zenodo is pending.
