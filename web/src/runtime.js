export function hardwareControlsEnabled(runtime) {
  return runtime?.studio_mode === "mock" || runtime?.hardware_writes_enabled === true;
}

export function isReadOnlyHardware(runtime) {
  return runtime?.studio_mode === "beacn" && !hardwareControlsEnabled(runtime);
}
