const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

export async function loadSolver(source) {
  let module;
  if (source instanceof WebAssembly.Module) {
    module = source;
  } else if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
    module = await WebAssembly.compile(source);
  } else if (typeof Response !== "undefined" && source instanceof Response) {
    module = await WebAssembly.compileStreaming(source);
  } else if (typeof source === "string") {
    const response = await fetch(source);
    module = await WebAssembly.compileStreaming(response);
  } else {
    throw new TypeError("loadSolver expects a URL, Response, ArrayBuffer, or WebAssembly.Module");
  }
  const instance = await WebAssembly.instantiate(module, {
    env: { dws_now_ms: () => Date.now() },
  });
  return createSolver(instance.exports);
}

export function createSolver(exports) {
  const allocBytes = (bytes) => {
    const ptr = exports.dws_alloc(bytes.length);
    if (!ptr) throw new Error("dws_alloc failed");
    new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
    return { ptr, len: bytes.length };
  };

  return {
    exports,
    solve(payload, request) {
      const payloadBytes = textEncoder.encode(JSON.stringify(payload));
      const requestBytes = textEncoder.encode(JSON.stringify(request));
      const p = allocBytes(payloadBytes);
      const r = allocBytes(requestBytes);
      try {
        const out = exports.dws_solve(p.ptr, p.len, r.ptr, r.len);
        if (!out) throw new Error("dws_solve returned null");
        const view = new DataView(exports.memory.buffer);
        const len = view.getUint32(out, true);
        const text = textDecoder.decode(
          new Uint8Array(exports.memory.buffer, out + 4, len),
        );
        exports.dws_dealloc(out, len + 4);
        const result = JSON.parse(text);
        if (result.error) throw new Error(result.error);
        return result;
      } finally {
        exports.dws_dealloc(p.ptr, p.len);
        exports.dws_dealloc(r.ptr, r.len);
      }
    },
  };
}
