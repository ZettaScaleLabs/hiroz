'use strict';

// Mocked github, context and core objects. The REST mocks honour page and
// per_page (default 30, as the API does), and paginate walks the pages, so a
// marker beyond the first page is invisible to code that does not paginate.

const ordinary = (i) => ({ user: { type: 'User' }, body: `comment ${i}` });
const botComment = (marker) => ({ user: { type: 'Bot' }, body: `text ${marker} text` });

function makeEnv({
  association = 'NONE',
  draft = false,
  reviews = [],
  comments = [],
  denyCreate = false,
} = {}) {
  const calls = [];
  const created = [];
  const log = { failed: [], warnings: [], notices: [], infos: [] };
  const store = [...comments];

  const page = (items) => (params = {}) => {
    const perPage = params.per_page ?? 30;
    const pageNo = params.page ?? 1;
    return { data: items().slice((pageNo - 1) * perPage, pageNo * perPage) };
  };

  const listReviews = async (params) => {
    calls.push('listReviews');
    return page(() => reviews)(params);
  };
  const listComments = async (params) => {
    calls.push('listComments');
    return page(() => store)(params);
  };
  const createComment = async ({ body }) => {
    calls.push('createComment');
    if (denyCreate) {
      const error = new Error('Resource not accessible by integration');
      error.status = 403;
      throw error;
    }
    created.push(body);
    store.push({ user: { type: 'Bot' }, body });
    return { data: {} };
  };

  const github = {
    rest: { pulls: { listReviews }, issues: { listComments, createComment } },
    paginate: async (fn, params = {}) => {
      const perPage = params.per_page ?? 30;
      const all = [];
      for (let pageNo = 1; ; pageNo += 1) {
        const { data } = await fn({ ...params, per_page: perPage, page: pageNo });
        all.push(...data);
        if (data.length < perPage) return all;
      }
    },
  };
  const context = {
    repo: { owner: 'owner', repo: 'repo' },
    payload: {
      pull_request: {
        number: 7,
        draft,
        author_association: association,
        user: { login: 'contributor' },
      },
    },
  };
  const core = {
    setFailed: (message) => log.failed.push(message),
    warning: (message) => log.warnings.push(message),
    notice: (message) => log.notices.push(message),
    info: (message) => log.infos.push(message),
  };
  return { github, context, core, calls, created, log };
}

module.exports = { makeEnv, ordinary, botComment };
