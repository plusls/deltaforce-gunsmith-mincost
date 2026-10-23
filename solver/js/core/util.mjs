export const asPlainObject = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};

export const asArray = (value) => (Array.isArray(value) ? value : []);

export const hasOwn = (object, key) =>
  Object.prototype.hasOwnProperty.call(object, key);

export function errorMessage(error) {
  return String(error && error.message ? error.message : error);
}
