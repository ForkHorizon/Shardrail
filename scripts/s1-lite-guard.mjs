import { lstatSync, realpathSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';

export const INITIAL_COMMIT_PREFIX = '1bb8838';

export function assertResultDirectory(resultDir, { shardrail, worktree, allowedResultDir }) {
  if ([shardrail, worktree].some((root) => resultDir === root || resultDir.startsWith(`${root}/`)) && resultDir !== allowedResultDir) {
    throw new Error('S1_RESULT_DIR must be outside the Shardrail checkouts');
  }
  return resultDir;
}

export function gitOutput(cwd, args, spawn = spawnSync) {
  const result = spawn('git', ['-C', cwd, ...args], { encoding: 'utf8' });
  if (result.status !== 0) {
    const detail = `${result.stderr || result.error?.message || `git ${args.join(' ')} failed`}`.trim();
    throw new Error(detail);
  }
  return result.stdout;
}

export function assertRepoIdentity({
  testRepo,
  shardrail,
  worktree,
  commitPrefix = INITIAL_COMMIT_PREFIX,
  lstat = lstatSync,
  realpath = realpathSync,
  git = (cwd, args) => gitOutput(cwd, args),
}) {
  const listed = lstat(testRepo);
  if (listed.isSymbolicLink()) throw new Error('refusing to reset a symlinked test repo');
  if (!listed.isDirectory()) throw new Error('s1-testrepo is not a directory');
  const gitLink = lstat(join(testRepo, '.git'));
  if (gitLink.isSymbolicLink()) throw new Error('refusing a symlinked .git');
  if (!gitLink.isDirectory()) throw new Error('refusing a gitdir file or non-directory .git');

  const gitDir = git(testRepo, ['rev-parse', '--git-dir']).trim();
  const resolvedGit = realpath(resolve(testRepo, gitDir));
  const expectedGit = realpath(join(testRepo, '.git'));
  if (resolvedGit !== expectedGit) throw new Error('s1-testrepo git directory is not its own .git');

  const repo = realpath(testRepo);
  if (repo === realpath(shardrail) || repo === realpath(worktree)) {
    throw new Error('refusing to reset the Shardrail checkout');
  }
  if (git(testRepo, ['remote']).trim()) throw new Error('refusing to reset a test repo that has a remote');
  if (git(testRepo, ['rev-parse', '--is-inside-work-tree']).trim() !== 'true') {
    throw new Error('s1-testrepo is not a git work tree');
  }
  const common = realpath(resolve(testRepo, git(testRepo, ['rev-parse', '--git-common-dir']).trim()));
  const shardrailCommon = realpath(resolve(shardrail, git(shardrail, ['rev-parse', '--git-common-dir']).trim()));
  if (common === shardrailCommon) throw new Error('s1-testrepo shares the Shardrail git directory');

  let commit;
  try {
    commit = git(testRepo, ['rev-parse', '--verify', `${commitPrefix}^{commit}`]).trim();
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`refusing to reset before ${commitPrefix} exists in s1-testrepo: ${detail}`);
  }
  if (!/^[0-9a-f]{40}$/.test(commit) || !commit.startsWith(commitPrefix)) {
    throw new Error(`refusing to reset: ${commitPrefix} resolved to ${commit}`);
  }
  return { repo, commit };
}

export function assertHeadIsCommit(head, commit) {
  const actual = head.trim();
  if (actual !== commit) throw new Error(`HEAD is ${actual}, expected ${commit}`);
}

const CODEX_HOME_MARKERS = ['~/.codex', '/.codex/', '~/.agents', '/.agents/'];

// Codex's own config-load warning (sent as configWarning and again as a thread warning)
// names the config file it parsed at startup. That is not the model reading ~/.codex, so
// it is the one notice left out of the check.
function isCodexConfigWarning(event) {
  if (event?.kind !== 'provider_notice') return false;
  const method = event?.payload?.method;
  if (method === 'configWarning') return true;
  return method === 'warning' && /unrecognized configuration setting/.test(String(event?.payload?.params?.message ?? ''));
}

// thread/started reports where Codex itself writes the session rollout
// (~/.codex/sessions/...). That is Codex's own log, not the model reading ~/.codex,
// so only that one field is dropped before the check.
function withoutRolloutPath(event) {
  const thread = event?.payload?.params?.thread;
  if (event?.kind !== 'provider_notice' || event?.payload?.method !== 'thread/started') return event;
  if (!thread || typeof thread.path !== 'string' || !/\/\.codex\/sessions\/[^/]+\/[^/]+\/[^/]+\/rollout-[^/]+\.jsonl$/.test(thread.path)) return event;
  const { path: _rollout, ...rest } = thread;
  return { ...event, payload: { ...event.payload, params: { ...event.payload.params, thread: rest } } };
}

export function codexHomePathHits(events) {
  const hits = [];
  for (const event of Array.isArray(events) ? events : []) {
    if (isCodexConfigWarning(event)) continue;
    const text = JSON.stringify(withoutRolloutPath(event));
    const markers = CODEX_HOME_MARKERS.filter((marker) => text.includes(marker));
    if (!markers.length) continue;
    const at = text.indexOf(markers[0]);
    hits.push({
      event_id: event?.event_id ?? null,
      kind: event?.kind ?? null,
      method: event?.payload?.method ?? null,
      markers,
      context: text.slice(Math.max(0, at - 160), at + 160),
    });
  }
  return hits;
}

export function turn1Failure(turn) {
  const denied = Array.isArray(turn.decisions) && turn.decisions.some((item) => item.decision === 'deny');
  if (!denied) return 'turn 1 never denied an approval';
  if (turn.allowed) {
    if (turn.outcome !== 'completed' && turn.outcome !== 'Provider') {
      return `turn 1 ended as ${turn.outcome}, expected completed`;
    }
    return null;
  }
  if (!turn.finished) return 'turn 1 did not finish after the denial';
  return null;
}
