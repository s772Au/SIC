import {join, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

const workspace = fileURLToPath(new URL('../../../', import.meta.url));

export function dshSourcePath() {
  const source = process.env.DSH_SOURCE_ROOT;
  if (!source) throw new Error('Set DSH_SOURCE_ROOT to the built DSH source checkout');
  return resolve(source);
}

export function fixturePath(...parts) {
  return resolve(process.env.SIC_TEST_BUILD_ROOT ?? join(workspace, '.local', 'dsh-fixtures'), ...parts);
}

export function evidencePath(...parts) {
  return resolve(process.env.SIC_TEST_EVIDENCE_ROOT ?? join(workspace, '.local', 'dsh-evidence'), ...parts);
}

export function testCoreBinary() {
  const executable = process.platform === 'win32' ? 'sic-core-rust.exe' : 'sic-core-rust';
  return resolve(process.env.SIC_CORE_TEST_BIN ?? join(process.env.CARGO_TARGET_DIR ?? join(workspace, 'target'), 'debug', executable));
}
