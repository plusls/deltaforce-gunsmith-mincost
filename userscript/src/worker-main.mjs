function startSolverWorker(scope) {
  let enginePromise = null;
  const payloadCache = new Map();

  scope.onmessage = async (event) => {
    const message = event.data;
    if (!message || typeof message !== "object") return;

    if (message.type === "init") {
      try {
        enginePromise = createMinCostEngine({ wasm: message.wasm });
        await enginePromise;
        scope.postMessage({ type: "ready" });
      } catch (error) {
        enginePromise = null;
        scope.postMessage({
          type: "init-error",
          message: String(error && error.message ? error.message : error),
        });
      }
      return;
    }

    if (message.type !== "solve") return;

    try {
      const engine = await enginePromise;
      if (!engine) throw new Error("solver worker not initialized");
      if (message.data) payloadCache.set(message.key, message.data);
      const data = payloadCache.get(message.key);
      if (!data) throw new Error(`missing payload for ${message.key}`);
      const context = { ...message.context, data };
      const result = engine.solve(context, message.options);
      scope.postMessage({ type: "result", id: message.id, result });
    } catch (error) {
      scope.postMessage({
        type: "error",
        id: message.id,
        message: String(error && error.message ? error.message : error),
      });
    }
  };
}

startSolverWorker(self);
