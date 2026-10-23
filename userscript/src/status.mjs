const STATUS_COLORS = Object.freeze({
  info: "#a2a5ad",
  ok: "#86efac",
  warn: "#fbbf24",
  error: "#f87171",
});

let statusEl = null;

export function setStatusEl(element) {
  statusEl = element;
}

export function statusColor(tone) {
  return STATUS_COLORS[tone] ?? STATUS_COLORS.info;
}

export function reportStatus(text, tone = "info") {
  if (!statusEl || !statusEl.isConnected) return;
  statusEl.textContent = text;
  statusEl.style.color = statusColor(tone);
}

export function friendlyError(error) {
  if (error && error.name === "UnsupportedConstraintError") {
    return `暂不支持这些约束: ${error.constraints.join(", ")}`;
  }
  if (error && error.name === "UnsupportedFeatureError") {
    if (error.feature === "preparedness") return "暂不支持战备下限约束";
    return `暂不支持: ${error.feature}`;
  }
  return String(error && error.message ? error.message : error);
}
