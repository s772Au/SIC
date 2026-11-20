"""Scoped, text-only SIC projection for an owned Codex Responses route.

The host admits archiveable user items and binds a Core client to the same
chain. The gateway owns neither Codex history nor Provider retry policy.
"""

from collections import Counter
from hashlib import sha256
from hmac import compare_digest
import json
import secrets
import sqlite3

from .native_span import closed_native_span


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()


def digest(value):
    return sha256(encoded(value)).hexdigest()


def user_item(text):
    return {'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': text}]}


def plain_user(item):
    if item.get('type') != 'message' or item.get('role') != 'user':
        return None
    blocks = item.get('content')
    if isinstance(blocks, list) and len(blocks) == 1 and blocks[0].get('type') == 'input_text':
        return blocks[0].get('text')
    return None


def edits_for(units, summary):
    return [{'op': 'COMPRESS', 'target': units[0]['unit_id'], 'content': summary},
            *({'op': 'DELETE', 'target': unit['unit_id']} for unit in units[1:])]


class CodexProjection:
    def __init__(self, path, thread_id, chain_id, operate, edit, *, batch=8, rollouts=None, include_native_span=False):
        if not thread_id or not chain_id or not 2 <= batch <= 32:
            raise ValueError('An owned Codex thread, Core chain and valid hierarchy batch are required')
        self.db = sqlite3.connect(path, check_same_thread=False)
        self.db.execute('PRAGMA journal_mode=WAL')
        self.db.execute('PRAGMA synchronous=FULL')
        self.db.executescript('''
            CREATE TABLE IF NOT EXISTS binding (singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                thread TEXT NOT NULL, chain TEXT NOT NULL, marker TEXT NOT NULL, batch INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS admitted (hash TEXT PRIMARY KEY, ordinal INTEGER NOT NULL UNIQUE,
                native_id TEXT UNIQUE);
            CREATE TABLE IF NOT EXISTS intent (base INTEGER PRIMARY KEY, hashes TEXT NOT NULL,
                source TEXT NOT NULL UNIQUE, source_digest TEXT NOT NULL, cb TEXT UNIQUE);
            CREATE TABLE IF NOT EXISTS checkpoint (token TEXT PRIMARY KEY, cut TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS retried (source TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS compacted_alias (wire_hash TEXT PRIMARY KEY, source_hash TEXT NOT NULL);
        ''')
        with self.db:
            self.db.execute('INSERT OR IGNORE INTO binding VALUES (1, ?, ?, ?, ?)',
                            (thread_id, chain_id, secrets.token_hex(24), batch))
        thread, chain, self.marker, saved_batch = self.db.execute(
            'SELECT thread, chain, marker, batch FROM binding').fetchone()
        if (thread, chain, saved_batch) != (thread_id, chain_id, batch):
            self.db.close()
            raise ValueError('Codex projection journal belongs to another binding or policy')
        self.thread_id, self.chain_id = thread, chain
        self.op, self.edit, self.batch = operate, edit, batch
        self.retry_source = None
        self.rollouts = rollouts
        self.rollout_offset = 0
        self.include_native_span = include_native_span

    def close(self):
        self.db.close()

    def admit_material(self, text):
        """Called by the trusted benchmark driver before it submits an archive segment."""
        if not isinstance(text, str) or not text.strip():
            raise ValueError('Archive material must be nonempty text')
        hash_ = digest(user_item(text))
        with self.db:
            self.db.execute('INSERT INTO admitted SELECT ?, coalesce(max(ordinal), 0) + 1, NULL FROM admitted', (hash_,))

    @staticmethod
    def alias_hash(item):
        if (set(item) != {'id', 'type', 'role', 'content'} or item.get('type') != 'message' or item.get('role') != 'user'
                or not isinstance(item.get('content'), list) or not item['content']
                or any(set(part) != {'type', 'text'} or part['type'] != 'input_text' or not isinstance(part['text'], str)
                       for part in item['content'] if isinstance(part, dict))
                or not all(isinstance(part, dict) for part in item['content'])):
            return None
        return digest(item)

    def refresh_compacted_aliases(self, by_id):
        if self.rollouts is None:
            return
        files = list(self.rollouts.rglob(f'rollout-*-{self.thread_id}.jsonl'))
        if len(files) != 1 or not files[0].resolve().is_relative_to(self.rollouts.resolve()):
            return
        with files[0].open('rb') as file:
            if self.rollout_offset == 0:
                first = json.loads(file.readline())
                if first.get('type') != 'session_meta' or first.get('payload', {}).get('id') != self.thread_id:
                    raise ValueError('Codex rollout identity is different from the gateway binding')
            else:
                file.seek(self.rollout_offset)
            while line := file.readline():
                if not line.endswith(b'\n'):
                    break
                entry = json.loads(line)
                self.rollout_offset = file.tell()
                if entry['type'] != 'compacted':
                    continue
                retained = entry['payload'].get('replacement_history') or []
                if not self.carrier_indices(retained):
                    continue
                covered = {key for row, in self.db.execute('SELECT hashes FROM intent WHERE cb IS NOT NULL')
                           for key in json.loads(row)}
                aliases = []
                for source_item in retained:
                    source = by_id.get(source_item.get('id'))
                    if source not in covered:
                        continue
                    wire_item = {key: value for key, value in source_item.items()
                                 if key != 'internal_chat_message_metadata_passthrough'}
                    wire_hash = self.alias_hash(wire_item)
                    if wire_hash:
                        aliases.append((wire_hash, source))
                with self.db:
                    self.db.executemany('INSERT OR IGNORE INTO compacted_alias VALUES (?, ?)', aliases)

    def material_keys(self, items, *, bind=False):
        allowed = dict(self.db.execute('SELECT hash, native_id FROM admitted'))
        by_id = {identity: key for key, identity in allowed.items() if identity is not None}
        found = []
        for i, item in enumerate(items):
            identity = item.get('id')
            text = plain_user(item)
            key = digest(user_item(text)) if isinstance(text, str) else None
            if isinstance(identity, str) and identity in by_id and key != by_id[identity]:
                actual = self.alias_hash(item)
                alias = self.db.execute('SELECT source_hash FROM compacted_alias WHERE wire_hash=?', (actual,)).fetchone()
                if alias is None and actual is not None:
                    self.refresh_compacted_aliases(by_id)
                    alias = self.db.execute('SELECT source_hash FROM compacted_alias WHERE wire_hash=?', (actual,)).fetchone()
                if alias != (by_id[identity],):
                    raise ValueError('Bound Codex item content or role changed')
                found.append((i, alias[0]))
                continue
            if key not in allowed:
                continue
            if set(item) != {'id', 'type', 'role', 'content'} or item['content'] != user_item(text)['content']:
                raise ValueError('Admitted Codex user item acquired unrecognized metadata')
            if not isinstance(identity, str) or not identity or (allowed[key] is not None and allowed[key] != identity):
                raise ValueError('Admitted Codex item has a changed native identity')
            if allowed[key] is None:
                if not bind:
                    raise ValueError('Compaction saw admitted text before its native Worker identity was bound')
                allowed[key] = identity
                with self.db:
                    self.db.execute('UPDATE admitted SET native_id=? WHERE hash=?', (identity, key))
            found.append((i, key))
        order = [key for _, key in found]
        if len(set(order)) != len(order):
            raise ValueError('Admitted Codex occurrence is duplicated')
        return found

    def check_request(self, thread_id, frame):
        if not compare_digest(thread_id or '', self.thread_id):
            raise ValueError('Unbound Codex thread')
        if any(frame.get(key) for key in ('previous_response_id', 'conversation')):
            raise ValueError('SIC Codex projection requires a stateless Responses request')
        items = frame.get('input')
        if not isinstance(items, list) or not all(isinstance(item, dict) for item in items):
            raise ValueError('Codex projection requires ordered Responses input items')
        return items

    def cut(self):
        return self.op('cut.get', {})

    def coverage(self, cut):
        leaves = dict(self.db.execute('SELECT cb, hashes FROM intent WHERE cb IS NOT NULL'))

        def expand(cb, level):
            if level == 1:
                if cb not in leaves:
                    raise ValueError('Selected leaf CB has no trusted Codex coverage')
                return json.loads(leaves[cb])
            detail = self.op('hierarchy.expand', {'cb_id': cb})
            members = detail['members']
            if not members or any(x['level'] != level - 1 for x in members):
                raise ValueError('Selected high CB has incomplete lineage')
            return [key for item in members for key in expand(item['cb_id'], level - 1)]

        selected = cut['view']['cbs']
        covered = [key for cb in selected for key in expand(cb['cb_id'], cb['level'])]
        published = [key for hashes, in self.db.execute('SELECT hashes FROM intent WHERE cb IS NOT NULL ORDER BY base')
                     for key in json.loads(hashes)]
        if covered != published or len(set(covered)) != len(covered):
            raise ValueError('Selected Codex cut does not cover the exact journaled interval')
        return covered

    def shared(self, cut):
        return [user_item(cb['body']) for cb in cut['view']['cbs']]

    def carrier_indices(self, items):
        receipts = {token for token, in self.db.execute('SELECT token FROM checkpoint')}
        result = []
        for i, item in enumerate(items):
            text = plain_user(item)
            # Codex adds its native prelude to the summary. The random marker
            # is never sent to the model and is accepted only in a full line.
            if text and any(line in receipts for line in text.splitlines()):
                result.append(i)
        return result

    def worker(self, thread_id, frame):
        items = self.check_request(thread_id, frame)
        if self.db.execute('SELECT 1 FROM intent WHERE cb IS NULL').fetchone():
            raise ValueError('SIC Codex compaction has an unsettled transaction')
        cut = self.cut()
        covered = self.coverage(cut)
        if not covered:
            self.material_keys(items, bind=True)
            return frame
        checkpoint = self.db.execute('SELECT token FROM checkpoint WHERE cut = ?', (cut['cut_id'],)).fetchone()
        if not checkpoint:
            raise ValueError('Selected Codex cut has no checkpoint receipt')
        carriers = self.carrier_indices(items)
        if not carriers or not any(checkpoint[0] in plain_user(items[i]).splitlines() for i in carriers):
            raise ValueError('Selected Codex cut has not reached the native request')
        ordered_items = self.material_keys(items, bind=True)
        positions = {key: index for index, key in enumerate(covered)}
        present = [(i, positions[key]) for i, key in ordered_items if key in positions]
        order = [rank for _, rank in present]
        if order != sorted(set(order)):
            raise ValueError('Archived Codex occurrence is duplicated or reordered')
        discarded = {i for i, _ in present} | set(carriers)
        first = min(discarded)
        output = []
        for i, item in enumerate(items):
            if i == first:
                output.extend(self.shared(cut))
            if i not in discarded:
                output.append(item)
        return {**frame, 'input': output}

    def drain(self):
        policy = {'context_id': 'main', 'min_level': 2, 'max_level': 3, 'min_sources_per_wb': 1,
                  'max_sources_per_wb': self.batch, 'max_units_per_wb': 512, 'max_actions': 32,
                  'level_batch_sizes': {'2': self.batch, '3': self.batch}, 'flush': False,
                  'editor_mode': 'host', 'max_model_calls': 0, 'program_rules': []}
        for _ in range(32):
            plan = self.op('runtime.schedule', policy)
            if plan.get('failure') or (plan.get('wait') or {}).get('code') == 'higher_volatile_binding':
                raise ValueError('Codex SIC hierarchy requires recovery')
            if not plan.get('host_jobs'):
                return self.cut()
            for job in plan['host_jobs']:
                for assignment in job['assignments']:
                    editor = assignment['editor']
                    claim = {'job_id': job['job_id'], 'editor': editor,
                             'claim_id': digest([self.thread_id, job['job_id'], editor])}
                    attempt = self.op('edit.host.claim', claim)
                    if not attempt['execute']:
                        if attempt['host_editor']['attempts'][editor]['status'] == 'completed':
                            continue
                        raise ValueError('Codex hierarchy Editor has an unknown outcome')
                    assigned = set(attempt['assigned_unit_ids'])
                    units = [unit for unit in attempt['units'] if unit['unit_id'] in assigned]
                    try:
                        summary = self.editor(self.cut(), units)
                        self.op('edit.host.submit', {**claim, 'ops': edits_for(units, summary), 'pvi': {}})
                    except Exception:
                        saved = self.op('edit.host.get', {'job_id': job['job_id'], 'editor': editor})
                        if saved['host_editor']['attempts'][editor]['status'] == 'completed':
                            continue
                        self.op('edit.host.fail', {**claim, 'reason': 'Unacknowledged Codex Editor delivery'})
                        raise
        raise ValueError('Codex SIC hierarchy did not settle')

    def editor(self, cut, units):
        if not units:
            raise ValueError('No authorized material for Codex Editor')
        summary = self.edit(self.shared(cut), units)
        if (not isinstance(summary, str) or not summary.strip()
                or len(summary.encode()) >= sum(len(unit['text'].encode()) for unit in units)):
            raise ValueError('Editor must deliver a nonempty shorter candidate')
        return summary

    def authorize_pending_retry(self):
        [source_id] = self.db.execute('SELECT source FROM intent WHERE cb IS NULL').fetchone()
        value = self.op('native.get', {'source_id': source_id})
        if value['version'] != 0 or value.get('publication'):
            raise ValueError('Previously completed Codex Editor must be resumed without retry')
        if self.db.execute('SELECT 1 FROM retried WHERE source=?', (source_id,)).fetchone():
            raise ValueError('The pending Codex Editor already used its one retry')
        self.retry_source = source_id

    def compact(self, thread_id, frame, instruction):
        items = self.check_request(thread_id, frame)
        if not items or plain_user(items[-1]) != instruction:
            raise ValueError('Missing authenticated host compaction instruction')
        pending = self.db.execute('SELECT base, hashes, source, source_digest FROM intent WHERE cb IS NULL').fetchone()
        if pending:
            candidate = self.op('native.get', {'source_id': pending[2]})
            if candidate.get('publication'):
                with self.db:
                    self.db.execute('UPDATE intent SET cb=? WHERE source=?',
                                    (candidate['publication']['publication']['cb_id'], pending[2]))
                pending = None
        cut = self.cut()
        covered = self.coverage(cut)
        admitted = [hash_ for hash_, in self.db.execute('SELECT hash FROM admitted ORDER BY ordinal')]
        if admitted[:len(covered)] != covered:
            raise ValueError('Archived items are not the oldest admitted Codex prefix')
        available = admitted[len(covered):]
        allowed = set(available)
        occurrences = [(key, i) for i, key in self.material_keys(items[:-1]) if key in allowed]
        matches = [key for key, _ in occurrences]
        if matches != available[:len(matches)] or len(set(matches)) != len(matches):
            raise ValueError('Native compaction lacks the ordered admitted prefix')
        if pending:
            base, previous, source_id, source_digest = pending
            if matches != json.loads(previous):
                raise ValueError('Retry does not match frozen Codex scope')
            if candidate['version'] == 0:
                if source_id != self.retry_source:
                    raise ValueError('Prior Codex Editor outcome is unknown')
                with self.db:
                    self.db.execute('INSERT INTO retried VALUES (?)', (source_id,))
                self.retry_source = None
                scoped = set(candidate['binding']['editable_unit_ids'])
                targets = [unit for unit in candidate['units'] if unit['unit_id'] in scoped]
        elif matches:
            base = cut['sequence']
            prefix = self.shared(cut)
            if self.include_native_span:
                span, anchors = closed_native_span(items[:-1], [i for _, i in occurrences], len(prefix))
            else:
                span = [items[i] for _, i in occurrences]
                anchors = [['input', i, 'content', 0, 'text'] for i in range(len(prefix), len(prefix) + len(span))]
            bindings = [{'cb_id': cb['cb_id'], 'native_anchors': [['input', i, 'content', 0, 'text']]}
                        for i, cb in enumerate(cut['view']['cbs'])]
            candidate = self.op('native.begin', {'format': 'openai-responses', 'editable_anchors': anchors,
                                'protected_cb_bindings': bindings, 'request': {'model': frame['model'],
                                    'input': prefix + span},
                                'idempotency_key': digest([self.thread_id, base, matches])})
            source_id, source_digest = candidate['source_id'], candidate['source_digest']
            with self.db:
                self.db.execute('INSERT INTO intent VALUES (?, ?, ?, ?, NULL)',
                                (base, json.dumps(matches), source_id, source_digest))
            targets = [unit for unit in candidate['units'] if unit['anchor'] in anchors]
        if pending or matches:
            if candidate['version'] == 0:
                summary = self.editor(cut, targets)
                candidate = self.op('native.commit', {'source_id': source_id, 'expected_source_digest': source_digest,
                    'expected_version': 0, 'edits': [{'unit_id': unit['unit_id'], 'expected_text': unit['text'],
                                                    'replacement': summary if i == 0 else ''}
                                                   for i, unit in enumerate(targets)],
                    'delivery': {'ops': edits_for(targets, summary), 'pvi': {}}})
            publication = candidate.get('publication')
            if not publication:
                result = self.op('native.publish', {'source_id': source_id, 'expected_source_digest': source_digest,
                    'expected_version': candidate['version'], 'expected_cut': base,
                    'idempotency_key': digest(['publish', source_id])})
                publication = result['publication']
            with self.db:
                self.db.execute('UPDATE intent SET cb=? WHERE source=?', (publication['publication']['cb_id'], source_id))
        final = self.drain()
        self.coverage(final)
        if not final['cut_id']:
            raise ValueError('Codex compaction has no published SIC cut')
        token = f'[SIC-CODEX-ADOPTED {self.marker} {final["cut_id"]}]'
        with self.db:
            self.db.execute('INSERT OR IGNORE INTO checkpoint VALUES (?, ?)', (token, final['cut_id']))
        return token
