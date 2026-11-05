'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { run, WARNING_MARKER, WELCOME_MARKER, FAILURE_MESSAGE } = require('../pr-draft-check.js');
const { makeEnv, ordinary, botComment } = require('./mocks.js');

const VERIFY = { notify: false, failOnViolation: true };
const NOTIFY = { notify: true, failOnViolation: false };
const BOTH = { notify: true, failOnViolation: true };

const maintainerReview = { author_association: 'MEMBER' };
const strangerReview = { author_association: 'NONE' };
const many = (n) => Array.from({ length: n }, (_, i) => ordinary(i));
const warnings = (env) => env.created.filter((b) => b.includes(WARNING_MARKER)).length;
const welcomes = (env) => env.created.filter((b) => b.includes(WELCOME_MARKER)).length;

test('draft returns early with no API call', async () => {
  const env = makeEnv({ draft: true });
  assert.equal(await run(env, BOTH), 'draft');
  assert.deepEqual(env.calls, []);
  assert.deepEqual(env.log.failed, []);
});

for (const association of ['OWNER', 'MEMBER', 'COLLABORATOR']) {
  test(`trusted author ${association} returns early with no API call`, async () => {
    const env = makeEnv({ association });
    assert.equal(await run(env, BOTH), 'trusted');
    assert.deepEqual(env.calls, []);
    assert.deepEqual(env.log.failed, []);
  });
}

test('maintainer review returns early: no failure, no comment', async () => {
  const env = makeEnv({ reviews: [maintainerReview] });
  assert.equal(await run(env, BOTH), 'reviewed');
  assert.deepEqual(env.log.failed, []);
  assert.deepEqual(env.created, []);
  assert.ok(!env.calls.includes('createComment'));
});

test('an untrusted reviewer still fails the check', async () => {
  const env = makeEnv({ reviews: [strangerReview] });
  assert.equal(await run(env, VERIFY), 'violation');
  assert.deepEqual(env.log.failed, [FAILURE_MESSAGE]);
});

test('verify mode fails the check and never touches comments', async () => {
  const env = makeEnv();
  assert.equal(await run(env, VERIFY), 'violation');
  assert.deepEqual(env.log.failed, [FAILURE_MESSAGE]);
  assert.ok(!env.calls.includes('listComments'));
  assert.ok(!env.calls.includes('createComment'));
});

test('notify mode creates the warning when none exists (empty thread)', async () => {
  const env = makeEnv({ association: 'CONTRIBUTOR' });
  assert.equal(await run(env, NOTIFY), 'violation');
  assert.equal(warnings(env), 1);
  assert.deepEqual(env.log.failed, []);
});

test('marker on page 1 suppresses creation', async () => {
  const env = makeEnv({
    association: 'CONTRIBUTOR',
    comments: [botComment(WARNING_MARKER), ...many(5)],
  });
  await run(env, NOTIFY);
  assert.equal(warnings(env), 0);
});

for (const position of [31, 100, 101]) {
  test(`marker at position ${position} suppresses creation`, async () => {
    const comments = [...many(position - 1), botComment(WARNING_MARKER)];
    const env = makeEnv({ association: 'CONTRIBUTOR', comments });
    await run(env, NOTIFY);
    assert.equal(warnings(env), 0);
  });
}

test('sequential eligible reruns post exactly one warning and one welcome', async () => {
  const env = makeEnv({ association: 'NONE', comments: many(130) });
  for (let i = 0; i < 3; i += 1) await run(env, NOTIFY);
  assert.equal(warnings(env), 1);
  assert.equal(welcomes(env), 1);
});

test('a denied createComment still ends in the policy failure', async () => {
  const env = makeEnv({ denyCreate: true });
  assert.equal(await run(env, BOTH), 'violation');
  assert.deepEqual(env.log.failed, [FAILURE_MESSAGE]);
  assert.ok(env.log.warnings.some((w) => w.includes('Resource not accessible')));
});

test('a denied createComment does not throw in notify mode', async () => {
  const env = makeEnv({ denyCreate: true });
  await assert.doesNotReject(run(env, NOTIFY));
});

test('welcome is decided without the verifier: NONE gets welcome and warning', async () => {
  const env = makeEnv({ association: 'NONE' });
  await run(env, NOTIFY);
  assert.equal(welcomes(env), 1);
  assert.equal(warnings(env), 1);
});

test('FIRST_TIME_CONTRIBUTOR gets welcome and warning', async () => {
  const env = makeEnv({ association: 'FIRST_TIME_CONTRIBUTOR' });
  await run(env, NOTIFY);
  assert.equal(welcomes(env), 1);
  assert.equal(warnings(env), 1);
});

test('other external associations get the warning but no welcome', async () => {
  const env = makeEnv({ association: 'CONTRIBUTOR' });
  await run(env, NOTIFY);
  assert.equal(welcomes(env), 0);
  assert.equal(warnings(env), 1);
});

test('welcome marker at position 31 suppresses a second welcome', async () => {
  const env = makeEnv({ comments: [...many(30), botComment(WELCOME_MARKER)] });
  await run(env, NOTIFY);
  assert.equal(welcomes(env), 0);
  assert.equal(warnings(env), 1);
});

test('a human comment quoting the marker does not suppress the warning', async () => {
  const env = makeEnv({ comments: [{ user: { type: 'User' }, body: WARNING_MARKER }] });
  await run(env, NOTIFY);
  assert.equal(warnings(env), 1);
});

test('welcome body has no indented lines (no code-block rendering)', () => {
  const { WELCOME_BODY } = require('../pr-draft-check.js');
  assert.ok(WELCOME_BODY.split('\n').every((line) => !/^ {4}/.test(line)));
});

test('welcome does not tell the contributor to mark ready before a maintainer review', () => {
  const { WELCOME_BODY } = require('../pr-draft-check.js');
  const lines = WELCOME_BODY.split('\n');
  const readyLine = lines.find((line) => /ready for review/i.test(line));
  assert.ok(readyLine, 'welcome must say when to mark the PR ready');
  assert.match(readyLine, /maintainer/i, 'the ready step must depend on a maintainer review');
  assert.doesNotMatch(
    WELCOME_BODY,
    /Mark as "Ready for review" when complete/,
    'ready must not follow local checks alone',
  );
});
