export function hardwareControlsEnabled(runtime) {
  return runtime?.studio_mode === "mock" || runtime?.hardware_writes_enabled === true;
}

export function linkControlsEnabled(runtime) {
  return runtime?.studio_mode === "mock"
    || runtime?.link_control_enabled === true
    || runtime?.hardware_writes_enabled === true;
}

export function isReadOnlyHardware(runtime) {
  return runtime?.studio_mode === "beacn" && !hardwareControlsEnabled(runtime);
}
