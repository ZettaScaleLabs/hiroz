'use strict';

// Decision and notification logic for the external-contributor PR check,
// called from .github/workflows/pr-draft-check.yml. It reads every pull
// request field from `context.payload` and never evaluates one.

const TRUSTED_ASSOCIATIONS = ['OWNER', 'MEMBER', 'COLLABORATOR'];
const WELCOME_ASSOCIATIONS = ['FIRST_TIME_CONTRIBUTOR', 'NONE'];

const WARNING_MARKER = 'External contributor PRs should be opened as DRAFT';
const WELCOME_MARKER = 'Welcome to hiroz!';

const FAILURE_MESSAGE =
  '❌ External contributor PRs should be opened as DRAFT until ready for final review.\n\n' +
  'Please either:\n' +
  '1. Convert this PR to a draft (see "Convert to draft" in the PR sidebar)\n' +
  '2. Wait for a maintainer to review before marking as ready\n\n' +
  'This helps us manage CI workflow approvals and prioritize reviews.\n' +
  'See CONTRIBUTING.md for details.';

const WELCOME_BODY = [
  `👋 ${WELCOME_MARKER} Thank you for your contribution.`,
  '',
  '**For external contributors**, we recommend opening PRs as **drafts** initially. This helps us manage CI workflow approvals efficiently.',
  '',
  '**To convert this PR to draft:**',
  '1. Click "Convert to draft" in the sidebar →',
  '2. Ensure all local checks pass (`./scripts/check-local.sh`)',
  '3. Mark as "Ready for review" when complete',
  '',
  'See [CONTRIBUTING.md][1] for details.',
  '',
  'A maintainer will review your PR soon!',
  '',
  '[1]: https://github.com/ZettaScaleLabs/hiroz/blob/main/CONTRIBUTING.md',
].join('\n');

// The "Verify external PR is draft or reviewed" step.
async function verifyStep({ github, context, core }) {
  const pr = context.payload.pull_request;
  if (pr.draft) {
    core.notice('✓ PR is a draft - this is the recommended workflow for external contributors.');
    return 'draft';
  }

  const reviews = await github.rest.pulls.listReviews({
    owner: context.repo.owner,
    repo: context.repo.repo,
    pull_number: pr.number,
  });
  if (reviews.data.some((review) => TRUSTED_ASSOCIATIONS.includes(review.author_association))) {
    core.notice('✓ PR has been reviewed by a maintainer.');
    return 'reviewed';
  }

  const comments = await github.rest.issues.listComments({
    owner: context.repo.owner,
    repo: context.repo.repo,
    issue_number: pr.number,
  });
  const alreadyCommented = comments.data.some(
    (comment) => comment.user.type === 'Bot' && comment.body.includes(WARNING_MARKER),
  );
  if (!alreadyCommented) {
    await github.rest.issues.createComment({
      owner: context.repo.owner,
      repo: context.repo.repo,
      issue_number: pr.number,
      body: FAILURE_MESSAGE,
    });
  }

  core.setFailed(FAILURE_MESSAGE);
  return 'violation';
}

// The "Post helpful comment for first-time contributors" step.
async function welcomeStep({ github, context }) {
  const pr = context.payload.pull_request;
  const comments = await github.rest.issues.listComments({
    owner: context.repo.owner,
    repo: context.repo.repo,
    issue_number: pr.number,
  });
  if (!comments.data.some((c) => c.user.type === 'Bot' && c.body.includes(WELCOME_MARKER))) {
    await github.rest.issues.createComment({
      owner: context.repo.owner,
      repo: context.repo.repo,
      issue_number: pr.number,
      body: WELCOME_BODY,
    });
  }
}

/**
 * Runs the two steps in the order the workflow does. A step after a failed
 * step is skipped, as Actions does for a step with no status-check function.
 * @returns {Promise<string>} trusted | draft | reviewed | violation
 */
async function run(env) {
  const pr = env.context.payload.pull_request;
  if (TRUSTED_ASSOCIATIONS.includes(pr.author_association)) return 'trusted';

  const verdict = await verifyStep(env);
  if (verdict !== 'violation' && !pr.draft && WELCOME_ASSOCIATIONS.includes(pr.author_association)) {
    await welcomeStep(env);
  }
  return verdict;
}

module.exports = { run, FAILURE_MESSAGE, WELCOME_BODY, WARNING_MARKER, WELCOME_MARKER };
