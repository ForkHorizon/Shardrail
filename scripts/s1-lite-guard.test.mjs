import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { assertHeadIsCommit, assertRepoIdentity, assertResultDirectory, codexHomePathHits, turn1Failure } from './s1-lite-guard.mjs';

const FULL = '1bb8838aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

function entry(kind) {
  return { isSymbolicLink: () => kind === 'symlink', isDirectory: () => kind === 'dir' };
}

function identity(overrides = {}) {
  const calls = [];
  const paths = new Map([
    ['/repo', '/repo'],
    ['/repo/.git', '/repo/.git'],
    ['/shardrail', '/shardrail'],
    ['/shardrail/.git', '/shardrail/.git'],
    ['/worktree', '/worktree'],
    ['/worktree/.git', '/worktree/.git'],
  ]);
  const kinds = new Map([
    ['/repo', 'dir'],
    ['/repo/.git', 'dir'],
  ]);
  const answers = new Map([
    ['/repo rev-parse --git-dir', '.git\n'],
    ['/repo remote', '\n'],
    ['/repo rev-parse --is-inside-work-tree', 'true\n'],
    ['/repo rev-parse --git-common-dir', '.git\n'],
    ['/repo rev-parse --verify 1bb8838^{commit}', `${FULL}\n`],
    ['/shardrail rev-parse --git-common-dir', '.git\n'],
  ]);
  const input = {
    testRepo: '/repo',
    shardrail: '/shardrail',
    worktree: '/worktree',
    lstat: (path) => entry(kinds.get(path) ?? 'missing'),
    realpath: (path) => paths.get(path) ?? path,
    git: (cwd, args) => {
      calls.push([cwd, ...args]);
      const key = `${cwd} ${args.join(' ')}`;
      if (!answers.has(key)) throw new Error(`unexpected git ${key}`);
      return answers.get(key);
    },
    ...overrides,
  };
  return { input, calls, paths, kinds, answers };
}

test('a real directory with its own .git and the known commit is accepted', () => {
  const { input, calls } = identity();
  assert.deepEqual(assertRepoIdentity(input), { repo: '/repo', commit: FULL });
  assert.equal(calls.some((call) => call.includes('reset')), false);
});

test('a parent symlink is accepted when the repo itself is a directory', () => {
  const { input, paths } = identity();
  input.testRepo = '/link/repo';
  input.lstat = (path) => entry(path === '/link/repo' || path === '/link/repo/.git' ? 'dir' : 'missing');
  paths.set('/link/repo', '/real/repo');
  paths.set('/link/repo/.git', '/real/repo/.git');
  input.git = (cwd, args) => {
    assert.equal(cwd, '/link/repo');
    if (args[0] === 'rev-parse' && args[1] === '--git-dir') return '.git\n';
    if (args[0] === 'remote') return '\n';
    if (args.join(' ') === 'rev-parse --is-inside-work-tree') return 'true\n';
    if (args.join(' ') === 'rev-parse --git-common-dir' && cwd === '/link/repo') return '.git\n';
    if (args.join(' ') === 'rev-parse --verify 1bb8838^{commit}') return `${FULL}\n`;
    throw new Error(`unexpected ${args.join(' ')}`);
  };
  const originalGit = input.git;
  input.git = (cwd, args) => {
    if (cwd === '/shardrail' && args.join(' ') === 'rev-parse --git-common-dir') return '.git\n';
    return originalGit(cwd, args);
  };
  assert.deepEqual(assertRepoIdentity(input), { repo: '/real/repo', commit: FULL });
});

test('a symlinked test repo is refused before git runs', () => {
  const { input, calls, kinds } = identity();
  kinds.set('/repo', 'symlink');
  assert.throws(() => assertRepoIdentity(input), /symlinked test repo/);
  assert.equal(calls.length, 0);
});

test('a symlinked .git is refused before git runs', () => {
  const { input, calls, kinds } = identity();
  kinds.set('/repo/.git', 'symlink');
  assert.throws(() => assertRepoIdentity(input), /symlinked \.git/);
  assert.equal(calls.length, 0);
});

test('a gitdir file is refused before git runs', () => {
  const { input, calls, kinds } = identity();
  kinds.set('/repo/.git', 'file');
  assert.throws(() => assertRepoIdentity(input), /gitdir file/);
  assert.equal(calls.length, 0);
});

test('a git dir outside the repo .git is refused before reset', () => {
  const { input, calls, answers, paths } = identity();
  answers.set('/repo rev-parse --git-dir', '/elsewhere/.git\n');
  paths.set('/elsewhere/.git', '/elsewhere/.git');
  assert.throws(() => assertRepoIdentity(input), /not its own \.git/);
  assert.equal(calls.some((call) => call.includes('reset')), false);
});

test('a missing target commit is refused and does not reset', () => {
  const { input, calls, answers } = identity();
  answers.delete('/repo rev-parse --verify 1bb8838^{commit}');
  assert.throws(() => assertRepoIdentity(input), /refusing to reset before 1bb8838/);
  assert.equal(calls.some((call) => call.includes('reset')), false);
});

test('a remote, a shared git dir, and the Shardrail checkout are refused', () => {
  const remote = identity();
  remote.answers.set('/repo remote', 'origin\n');
  assert.throws(() => assertRepoIdentity(remote.input), /remote/);

  const shared = identity();
  shared.answers.set('/repo rev-parse --git-common-dir', '/shardrail/.git\n');
  shared.paths.set('/shardrail/.git', '/shardrail/.git');
  assert.throws(() => assertRepoIdentity(shared.input), /shares the Shardrail git directory/);

  const checkout = identity();
  checkout.paths.set('/repo', '/shardrail');
  assert.throws(() => assertRepoIdentity(checkout.input), /Shardrail checkout/);
});

test('HEAD after reset must be the verified full hash', () => {
  assert.doesNotThrow(() => assertHeadIsCommit(`${FULL}\n`, FULL));
  assert.throws(() => assertHeadIsCommit('1bb8838deadbeef', FULL), /expected 1bb8838a/);
});

test('result directory allows only the designated ignored live report under Shardrail', () => {
  const roots = { shardrail: '/shardrail', worktree: '/worktree', allowedResultDir: '/shardrail/spikes/reviews19/live-1' };
  assert.equal(assertResultDirectory(roots.allowedResultDir, roots), roots.allowedResultDir);
  assert.equal(assertResultDirectory('/tmp/s1-live', roots), '/tmp/s1-live');
  assert.throws(() => assertResultDirectory('/shardrail/spikes/reviews19/live-2', roots), /outside the Shardrail checkouts/);
  assert.throws(() => assertResultDirectory('/worktree/live', roots), /outside the Shardrail checkouts/);
});

test('turn 1 can finish after a denial without a second approval', () => {
  assert.match(turn1Failure({ decisions: [], allowed: false, finished: true, outcome: 'completed' }), /never denied/);
  assert.equal(turn1Failure({ decisions: [{ decision: 'deny' }], allowed: true, finished: true, outcome: 'completed' }), null);
  assert.equal(turn1Failure({ decisions: [{ decision: 'deny' }], allowed: true, finished: true, outcome: 'Provider' }), null);
  assert.match(turn1Failure({ decisions: [{ decision: 'deny' }], allowed: true, finished: true, outcome: 'interrupted' }), /expected completed/);
  assert.equal(turn1Failure({ decisions: [{ decision: 'deny' }], allowed: false, finished: true, outcome: 'failed' }), null);
  assert.match(turn1Failure({ decisions: [{ decision: 'deny' }], allowed: false, finished: false, outcome: null }), /did not finish/);
});

test('journaled commands, approvals, items, and paths must stay out of Codex and agent homes', () => {
  assert.deepEqual(codexHomePathHits([{ event_id: 'ok', kind: 'command', payload: { argv: ['ls'] } }]), []);
  const hits = codexHomePathHits([
    { event_id: 'skill', kind: 'command', payload: { command: 'cat ~/.codex/skills/SKILL.md' } },
    { event_id: 'plugin', kind: 'approval', payload: { path: '/Users/owner/.codex/plugins/cache/ponytail/SKILL.md' } },
    { event_id: 'agents', kind: 'item', payload: { file: '/Users/owner/.agents/skills/demo' } },
    { event_id: 'tilde-agents', kind: 'file', payload: { file: '~/.agents/skills/demo' } },
  ]);
  assert.deepEqual(hits.map((hit) => hit.event_id), ['skill', 'plugin', 'agents', 'tilde-agents']);
  assert.deepEqual(hits[0].markers, ['~/.codex', '/.codex/']);
  assert.deepEqual(hits[1].markers, ['/.codex/']);
  assert.deepEqual(hits[2].markers, ['/.agents/']);
  assert.deepEqual(hits[3].markers, ['~/.agents', '/.agents/']);
});

test('the live script gates on the env and resets only through the guard', () => {
  const source = readFileSync(new URL('./s1-lite-live.mjs', import.meta.url), 'utf8');
  assert.match(source, /SHARDRAIL_S1_LIVE !== '1'/);
  assert.match(source, /assertRepoIdentity\(/);
  assert.match(source, /turn1Failure\(/);
  assert.match(source, /--allow-session-create/);
  assert.match(source, /New session/);
  assert.match(source, /codexHomePathHits\(/);
  const guardAt = source.indexOf('assertRepoIdentity(');
  const resetAt = source.indexOf("'reset', '--hard'");
  assert.ok(guardAt > 0 && resetAt > guardAt);
});
