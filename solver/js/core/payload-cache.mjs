export const PAYLOAD_CACHE_LIMIT = 4;

export class BoundedPayloadCache extends Map {
  constructor(limit) {
    super();
    this.limit = limit;
  }

  get(key) {
    if (!super.has(key)) return undefined;
    const value = super.get(key);
    super.delete(key);
    super.set(key, value);
    return value;
  }

  set(key, value) {
    if (super.has(key)) super.delete(key);
    super.set(key, value);
    while (this.size > this.limit) super.delete(super.keys().next().value);
    return this;
  }
}

export function fillCachedPayload(message, cache) {
  if (!message || typeof message !== "object") return message;
  if (message.data != null) return message;
  if (message.dataKey == null || !cache.has(message.dataKey)) return message;
  return { ...message, data: cache.get(message.dataKey) };
}
