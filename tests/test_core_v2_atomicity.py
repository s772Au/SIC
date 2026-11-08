"""Publication fault injection: each persistent write must roll back as one cut."""
import unittest
from unittest.mock import patch

from sic_core import Core


class PublicationAtomicityTests(unittest.TestCase):
    def prepare(self):
        core = Core()
        context = {"actor": "host", "allow_write": True, "allow_model": False}

        def call(operation, payload=None):
            return core.operate(operation, context, payload or {})["result"]

        context["chain_id"] = call("chain.create")["chain_id"]
        source = call("source.ingest", {"request": {"messages": [{"role": "user", "content": "retained source"}]}})
        job = call("edit.program", {"source_id": source["source_id"],
                                     "pvi": {"P": ["fixed evidence"], "V": ["next WB clue"],
                                             "I": {"display": {"state": "current"}}}})
        payload = {"job_id": job["job_id"], "expected_cut": 0, "idempotency_key": "one-publication"}
        return core, call, payload

    @staticmethod
    def snapshot(core):
        return list(core.store.db.iterdump())

    def test_every_publication_write_is_atomic_and_retryable(self):
        # Discover the actual write count, rather than bake in a schema layout.
        probe, invoke, payload = self.prepare()
        writes = []
        originals = {name: getattr(probe.store, name) for name in ("put", "save", "remember")}

        def recording(name):
            def run(*args, **kwargs):
                writes.append(name)
                return originals[name](*args, **kwargs)
            return run

        with patch.multiple(probe.store, **{name: recording(name) for name in originals}):
            invoke("cut.publish", payload)
        probe.store.db.close()
        self.assertGreaterEqual(len(writes), 4)

        for fail_at in range(1, len(writes) + 1):
            with self.subTest(persistent_write=fail_at, total=len(writes)):
                core, call, payload = self.prepare()
                before = self.snapshot(core)
                counter = [0]
                methods = {name: getattr(core.store, name) for name in originals}

                def injecting(name):
                    def run(*args, **kwargs):
                        counter[0] += 1
                        result = methods[name](*args, **kwargs)
                        if counter[0] == fail_at:
                            raise RuntimeError("injected persistence failure after write")
                        return result
                    return run

                with patch.multiple(core.store, **{name: injecting(name) for name in methods}):
                    with self.assertRaises(RuntimeError):
                        call("cut.publish", payload)
                self.assertEqual(before, self.snapshot(core), "partial cut survived rollback")
                first = call("cut.publish", payload)
                after = self.snapshot(core)
                self.assertEqual(first, call("cut.publish", payload))
                self.assertEqual(after, self.snapshot(core), "retry duplicated durable state")
                view = call("cut.get")["view"]
                self.assertEqual(len(view["cbs"]), 1)
                self.assertIn("fixed evidence", view["cbs"][0]["body"])
                self.assertEqual(view["instant"]["display"], {"state": "current"})
                self.assertEqual(view["pending"][0]["units"][0]["text"], "next WB clue")
                core.store.db.close()

    def test_peer_can_acknowledge_its_inherited_current_cut(self):
        core, call, payload = self.prepare()
        try:
            published = call("cut.publish", payload)
            checkpoint = call("chain.checkpoint")
            branch = call("chain.fork", {"checkpoint_id": checkpoint["checkpoint_id"]})
            context = {"actor": "host", "chain_id": branch["chain_id"],
                       "allow_write": True, "allow_model": False}
            current = core.operate("cut.get", context, {})["result"]
            ack = core.operate("cut.ack", context, {"cut_id": current["cut_id"],
                                                    "digest": current["cut"]["digest"]})["result"]
            self.assertTrue(ack["acknowledged"])
            self.assertTrue(ack["current"])
            self.assertEqual(call("cut.get")["cut_id"], published["cut_id"])
        finally:
            core.store.db.close()


if __name__ == "__main__":
    unittest.main()
