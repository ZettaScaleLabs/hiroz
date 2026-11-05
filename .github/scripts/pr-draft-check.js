'use strict';

// Policy and notification logic for the external-contributor PR check.
// Used by pr-draft-check.yml (verifier) and pr-draft-notify.yml (comment poster).
// PR fields come from `context.payload` and are only data, never evaluated.

const TRUSTED_ASSOCIATIONS = ['OWNER', 'MEMBER', 'COLLABORATOR'];
const WELCOME_ASSOCIATIONS = ['FIRST_TIME_CONTRIBUTOR', 'NONE'];

const WARNING_MARKER = 'External contributor PRs should be opened as DRAFT';
const WELCOME_MARKER = 'Welcome to hiroz!';

const FAILURE_MESSAGE = [
  `❌ ${WARNING_MARKER} until ready for final review.`,
  '',
  'Please either:',
  '1. Convert this PR to a draft (see "Convert to draft" in the PR sidebar)',
  '2. Wait for a maintainer to review before marking as ready',
  '',
  'This helps us manage CI workflow approvals and prioritize reviews.',
  'See CONTRIBUTING.md for details.',
].join('\n');

const WELCOME_BODY = [
  `👋 ${WELCOME_MARKER} Thank you for your contribution.`,
  '',
  '**For external contributors**, we recommend opening PRs as **drafts** initially. This helps us manage CI workflow approvals efficiently.',
  '',
  '**To convert this PR to draft:**',
  '1. Click "Convert to draft" in the sidebar →',
  '2. Ensure all local checks pass (`./scripts/check-local.sh`)',
  '3. Keep it a draft until a maintainer has reviewed it, then mark it "Ready for review"',
  '',
  'See [CONTRIBUTING.md][1] for details.',
  '',
  'A maintainer will review your PR soon!',
  '',
  '[1]: https://github.com/ZettaScaleLabs/hiroz/blob/main/CONTRIBUTING.md',
].join('\n');

// Search every page. A larger per_page only moves the boundary.
async function hasBotComment({ github, context, prNumber, marker }) {
  const comments = await github.paginate(github.rest.issues.listComments, {
    owner: context.repo.owner,
    repo: context.repo.repo,
    issue_number: prNumber,
    per_page: 100,
  });
  return comments.some(
    (comment) =>
      comment.user &&
      comment.user.type === 'Bot' &&
      typeof comment.body === 'string' &&
      comment.body.includes(marker),
  );
}

// Post `body` once. A list or post failure is logged, not thrown, so it cannot hide the verdict.
async function postOnce({ github, context, core, prNumber, marker, body }) {
  try {
    if (await hasBotComment({ github, context, prNumber, marker })) {
      core.info(`A comment with "${marker}" exists; not posting another.`);
      return;
    }
    await github.rest.issues.createComment({
      owner: context.repo.owner,
      repo: context.repo.repo,
      issue_number: prNumber,
      body,
    });
  } catch (error) {
    core.warning(`Could not post the "${marker}" comment: ${error.message}`);
  }
}

/**
 * @param {object} args github, context and core, as given by actions/github-script
 * @param {object} options
 * @param {boolean} options.notify post the warning and welcome comments
 * @param {boolean} options.failOnViolation call core.setFailed on a policy violation
 * @returns {Promise<string>} trusted | draft | reviewed | violation
 */
async function run({ github, context, core }, { notify, failOnViolation }) {
  const pr = context.payload.pull_request;
  const association = pr.author_association;

  if (TRUSTED_ASSOCIATIONS.includes(association)) {
    core.info(`Author association ${association} is trusted.`);
    return 'trusted';
  }

  if (pr.draft) {
    core.notice('✓ PR is a draft - this is the recommended workflow for external contributors.');
    return 'draft';
  }

  const reviews = await github.paginate(github.rest.pulls.listReviews, {
    owner: context.repo.owner,
    repo: context.repo.repo,
    pull_number: pr.number,
    per_page: 100,
  });
  if (reviews.some((review) => TRUSTED_ASSOCIATIONS.includes(review.author_association))) {
    core.notice('✓ PR has been reviewed by a maintainer.');
    return 'reviewed';
  }

  if (notify) {
    // The two comments are independent of each other.
    if (WELCOME_ASSOCIATIONS.includes(association)) {
      await postOnce({
        github, context, core, prNumber: pr.number,
        marker: WELCOME_MARKER, body: WELCOME_BODY,
      });
    }
    await postOnce({
      github, context, core, prNumber: pr.number,
      marker: WARNING_MARKER, body: FAILURE_MESSAGE,
    });
  }

  if (failOnViolation) {
    core.setFailed(FAILURE_MESSAGE);
  }
  return 'violation';
}

module.exports = { run, FAILURE_MESSAGE, WELCOME_BODY, WARNING_MARKER, WELCOME_MARKER };
