# SIC v0.1 experimental materials

This supplement accompanies the [v1 preprint](https://doi.org/10.5281/zenodo.24051713) and [GitHub v0.1](https://github.com/s772Au/SIC/releases/tag/v0.1).

Demo: [YouTube](https://youtu.be/s0I9LJ9-By8) · [Bilibili](https://www.bilibili.com/video/BV1TTWn6aERh/).

## Contents and evidence boundary

- `results/` contains field-selected public summaries derived from recorded checks, with hashes of their private source reports. Source hashes identify provenance; they do not make unpublished raw sessions independently replayable.
- `../dsh-live-smoke/` contains three newly created synthetic tasks, seven fixture files, frozen answers and scoring rules. These synthetic tasks retain CC0-1.0 as declared in `tasks.json`.
- `../compression/` contains editing/compression controls, prompts, frozen schemas, dependency/model/source locks and preparation instructions. Third-party corpora and model weights must be obtained separately under their own licenses.
- The full source archive includes executable runners, verifiers, Core and integration fixtures. The separate experiment archive is an overlay: extract it into the corresponding full source archive root. It is not a standalone installation.
- No original lost experiment is reconstructed or claimed. No private credentials, session databases, raw model responses, private working drafts, downloaded datasets or weights are included.

## Reproduce checks and run new experiments

From the full source root, create an isolated Python environment and follow `docs/python-core-package.md` and `research/compression/README.md` for pinned optional dependencies. Run `python -m unittest discover -s tests -p "test_*.py"` for the local unit suite. Optional dependency tests can skip unless their documented SDK/processor fixtures are installed; the recorded 273-test result used its explicit frozen dependencies and owned loopback services, not a real model.

Use `python scripts/verify_dsh_live_run.py prepare --output <new-directory>` to prepare the synthetic tasks. Use `python scripts/run_dsh_live_tests.py --help` and `python scripts/verify_dsh_live_run.py verify --help` for the isolated real DSH runner and read-only verifier. Real runs require a separately installed DSH and explicitly configured provider credentials in the host environment. Do not put credentials into archives. The runner needs integration helpers from the full source archive, including `integrations/tests/native_peer_faults.py`.

Freeze the implementation, prompts, physical model/revision where available, tokenizer, tasks, prices and provider routing before a comparison. Retain all failures. `unchanged` uses program KEEP through the SIC bridge; it is not bare DSH without SIC. `separate` and `shared_i` represent different request layouts.

The released exploratory runs used a nonfixed gateway: g completed 3 tasks with 2 exact answers; h completed 2 exact tasks and failed the third without a quality score; i failed after partial shared-prefix wire validation. These are not controlled proof of superiority, physical cache reuse, cost parity or a safe recursion depth. Usage/cost unknowns remain unknown. Details: `docs/dsh-live-results-20261108.md`.

## Licenses

Original executable code: MIT. Original result summaries and this scientific documentation: CC BY 4.0. Synthetic tasks: CC0-1.0. Paper: CC BY 4.0. External dependencies, datasets and model weights retain their respective licenses; downloading an archive does not relicense them.
