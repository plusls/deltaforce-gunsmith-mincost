import {
  BoundedPayloadCache,
  PAYLOAD_CACHE_LIMIT,
} from "../../solver/js/core/payload-cache.mjs";
import { WORKER_MESSAGE_TYPES } from "../../solver/js/core/protocol.mjs";
import { errorMessage } from "../../solver/js/core/util.mjs";

function startSolverWorker(scope) {
  let enginePromise = null;
  const payloadCache = new BoundedPayloadCache(PAYLOAD_CACHE_LIMIT);

  scope.onmessage = async (event) => {
    const message = event.data;
    if (!message || typeof message !== "object") return;

    if (message.type === WORKER_MESSAGE_TYPES.INIT) {
      try {
        enginePromise = createMinCostEngine({ wasm: message.wasm });
        await enginePromise;
        scope.postMessage({ type: WORKER_MESSAGE_TYPES.READY });
      } catch (error) {
        enginePromise = null;
        scope.postMessage({
          type: WORKER_MESSAGE_TYPES.INIT_ERROR,
          message: errorMessage(error),
        });
      }
      return;
    }

    if (message.type !== WORKER_MESSAGE_TYPES.SOLVE) return;

    try {
      const engine = await enginePromise;
      if (!engine) throw new Error("solver worker not initialized");
      if (message.data) payloadCache.set(message.key, message.data);
      const data = payloadCache.get(message.key);
      if (!data) throw new Error(`missing payload for ${message.key}`);
      const context = { ...message.context, data };
      const result = engine.solve(context, message.options);
      scope.postMessage({ type: WORKER_MESSAGE_TYPES.RESULT, id: message.id, result });
    } catch (error) {
      scope.postMessage({
        type: WORKER_MESSAGE_TYPES.ERROR,
        id: message.id,
        message: errorMessage(error),
      });
    }
  };
}

startSolverWorker(self);
