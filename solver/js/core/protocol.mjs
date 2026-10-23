export const WORKER_MESSAGE_TYPES = Object.freeze({
  INIT: "init",
  READY: "ready",
  INIT_ERROR: "init-error",
  SOLVE: "solve",
  RESULT: "result",
  ERROR: "error",
});

export const WORKER_MESSAGE_FIELDS = Object.freeze({
  TYPE: "type",
  ID: "id",
  KEY: "key",
  DATA: "data",
  CONTEXT: "context",
  OPTIONS: "options",
  RESULT: "result",
  MESSAGE: "message",
  WASM: "wasm",
});

export const SITE_MESSAGE_PURPOSE = Object.freeze({
  SOLVE: "solve",
  SEARCH: "search",
});

export const DEFAULT_SITE_MODE = "operations";

export const SITE_OPTIMIZER_WORKER_URL_PATTERN = /optimizer\.worker/;

export const SITE_SOLVER_URL_HINT_PATTERN = /optimizer|solver/i;
