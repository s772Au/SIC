"""Admit occurrences from the bound native Codex session instead of a benchmark importer."""

from .sic_responses import CodexProjection, digest, plain_user, user_item


def native_environment(text):
    if not isinstance(text, str):
        return False
    lines = text.strip().splitlines()
    return (len(lines) > 2 and lines[0] == '<environment_context>' and lines[-1] == '</environment_context>'
            and any(line.lstrip().startswith('<cwd>') for line in lines[1:-1]))


class CodexNativeProjection(CodexProjection):
    def __init__(self, path, thread_id, chain_id, operate, edit, *, batch=8, rollouts=None):
        super().__init__(path, thread_id, chain_id, operate, edit, batch=batch,
                         rollouts=rollouts, include_native_span=True)
        self.db.execute('CREATE TABLE IF NOT EXISTS native_user (key TEXT PRIMARY KEY, body TEXT NOT NULL)')
        if self.db.execute('SELECT 1 FROM admitted WHERE hash NOT IN (SELECT key FROM native_user) LIMIT 1').fetchone():
            self.close()
            raise ValueError('The native Codex session needs its own journal')

    def admit_material(self, _):
        raise ValueError('Native Codex sessions admit their own authenticated Worker occurrences')

    def material_keys(self, items, *, bind=False):
        carriers = set(self.carrier_indices(items))
        known = {identity: (key, body) for identity, key, body in self.db.execute(
            'SELECT native_id, hash, body FROM admitted JOIN native_user ON admitted.hash = native_user.key')}
        found = []
        for index, item in enumerate(items):
            if index in carriers:
                continue
            identity = item.get('id')
            if not isinstance(identity, str) or not identity:
                continue
            text = plain_user(item)
            key, body = known.get(identity, (None, None))
            shape = set(item) == {'id', 'type', 'role', 'content'}
            if key:
                if not shape or body != digest(user_item(text)):
                    wire = self.alias_hash(item)
                    prior = self.db.execute('SELECT source_hash FROM compacted_alias WHERE wire_hash=?', (wire,)).fetchone()
                    if prior is None and wire:
                        self.refresh_compacted_aliases({native: v[0] for native, v in known.items()})
                        prior = self.db.execute('SELECT source_hash FROM compacted_alias WHERE wire_hash=?', (wire,)).fetchone()
                    if prior != (key,):
                        raise ValueError('Bound Codex item content or role changed')
                found.append((index, key))
                continue
            if not bind or not shape or not isinstance(text, str) or not text.strip() or native_environment(text):
                continue
            key = digest(['codex-native-user', self.thread_id, identity])
            body = digest(user_item(text))
            with self.db:
                self.db.execute('INSERT INTO admitted SELECT ?, coalesce(max(ordinal), 0) + 1, ? FROM admitted',
                                (key, identity))
                self.db.execute('INSERT INTO native_user VALUES (?, ?)', (key, body))
            known[identity] = (key, body)
            found.append((index, key))
        order = [key for _, key in found]
        if len(set(order)) != len(order):
            raise ValueError('Duplicate native Codex occurrence')
        return found
