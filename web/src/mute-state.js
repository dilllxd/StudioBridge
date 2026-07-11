export function isMixMuted(state, mix) {
  return state === "muted_all" || state === `muted_${mix}`;
}

export function toggleMixMute(state, mix) {
  const personal = isMixMuted(state, "personal") !== (mix === "personal");
  const audience = isMixMuted(state, "audience") !== (mix === "audience");
  if (personal && audience) return "muted_all";
  if (personal) return "muted_personal";
  if (audience) return "muted_audience";
  return "unmuted";
}
