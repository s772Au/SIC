import contextlib,json,sqlite3,threading,uuid
from .adapters import digest

def uid(prefix):return prefix+'_'+uuid.uuid4().hex

def dumps(value):return json.dumps(value,ensure_ascii=False,sort_keys=True,separators=(',',':'),allow_nan=False)

class Store:
    def __init__(self,path):
        self.lock=threading.RLock()
        self.db=sqlite3.connect(path,check_same_thread=False,isolation_level=None)
        self.db.execute('PRAGMA journal_mode=WAL');self.db.execute('PRAGMA foreign_keys=ON');self.db.execute('PRAGMA busy_timeout=10000')
        self.db.executescript('''
        CREATE TABLE IF NOT EXISTS objects(id TEXT PRIMARY KEY,kind TEXT NOT NULL,data TEXT NOT NULL,digest TEXT NOT NULL);
        CREATE TRIGGER IF NOT EXISTS immutable_objects_update BEFORE UPDATE ON objects BEGIN SELECT RAISE(ABORT,'immutable object'); END;
        CREATE TRIGGER IF NOT EXISTS immutable_objects_delete BEFORE DELETE ON objects BEGIN SELECT RAISE(ABORT,'immutable object'); END;
        CREATE TABLE IF NOT EXISTS boards(id TEXT PRIMARY KEY,state TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS chains(id TEXT PRIMARY KEY,state TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS revocations(id TEXT PRIMARY KEY,record TEXT NOT NULL);
        CREATE TRIGGER IF NOT EXISTS immutable_revocations_update BEFORE UPDATE ON revocations BEGIN SELECT RAISE(ABORT,'immutable revocation'); END;
        CREATE TRIGGER IF NOT EXISTS immutable_revocations_delete BEFORE DELETE ON revocations BEGIN SELECT RAISE(ABORT,'immutable revocation'); END;
        CREATE TABLE IF NOT EXISTS receipts(key TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,result TEXT NOT NULL);
        ''')
    @contextlib.contextmanager
    def transaction(self):
        with self.lock:
            self.db.execute('BEGIN IMMEDIATE')
            try:yield;self.db.execute('COMMIT')
            except BaseException:self.db.execute('ROLLBACK');raise
    def put(self,kind,value,object_id=None):
        object_id=object_id or uid(kind);self.db.execute('INSERT INTO objects VALUES(?,?,?,?)',(object_id,kind,dumps(value),digest(value)));return object_id
    def revoked(self,object_id):return self.db.execute('SELECT 1 FROM revocations WHERE id=?',(object_id,)).fetchone() is not None
    def get(self,object_id,kind=None):
        if self.revoked(object_id):raise ValueError('object access revoked')
        row=self.db.execute('SELECT kind,data,digest FROM objects WHERE id=?',(object_id,)).fetchone()
        if row is None or kind and row[0]!=kind:raise ValueError('object not found or wrong kind')
        data=json.loads(row[1])
        if row[0]=='cut' and any(self.revoked(x) for group in data.get('groups',{}).values() for x in group):raise ValueError('checkpoint cut has revoked dependency')
        if row[0]=='checkpoint':self.get(data['cut_id'],'cut')
        if row[0]=='cb' and any(self.revoked(x) for x in data.get('source_ids',[])):raise ValueError('CB access revoked by source dependency')
        if row[0]=='source':
            for dependency in data.get('dependencies',[]):self.get(dependency,'source')
        if row[0]=='cb':
            for source_id in data.get('source_ids',[]):self.get(source_id,'source')
        if digest(data)!=row[2]:raise ValueError('object integrity mismatch')
        return data
    def state(self,chain_id):
        row=self.db.execute('SELECT state FROM chains WHERE id=?',(chain_id,)).fetchone()
        if not row:raise ValueError('chain not found')
        return json.loads(row[0])
    def save(self,state):self.db.execute('INSERT INTO chains VALUES(?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state',(state['chain_id'],dumps(state)))
    def receipt(self,key,fingerprint):
        row=self.db.execute('SELECT fingerprint,result FROM receipts WHERE key=?',(key,)).fetchone()
        if not row:return None
        if row[0]!=fingerprint:raise ValueError('idempotency key reused with different operation')
        return json.loads(row[1])
    def remember(self,key,fingerprint,value):self.db.execute('INSERT INTO receipts VALUES(?,?,?)',(key,fingerprint,dumps(value)))
