// Invoked only by the local launcher. The credential is returned over the
// private stdout pipe; no credential value is accepted in process arguments.
const { createRequire } = require('node:module');
const { readFileSync } = require('node:fs');

try {
  if (process.argv.length !== 5) process.exit(2);
  const [, , cliPath, credentialFile, reference] = process.argv;
  const dshRequire = createRequire(cliPath);
  const { parseCredentialsDocument } = dshRequire('@deepseek-ai/dsh-credentials-local');
  if (typeof parseCredentialsDocument !== 'function') process.exit(2);
  const document = parseCredentialsDocument(readFileSync(credentialFile, 'utf8'), '<DSH credential store>');
  const value = document.refs.get(reference);
  if (typeof value !== 'string' || value.length === 0 || value.length > 4096 || /[\r\n]/.test(value)) process.exit(2);
  process.stdout.write(value);
} catch {
  // Parser diagnostics may contain source text; never emit them here.
  process.exitCode = 2;
}
