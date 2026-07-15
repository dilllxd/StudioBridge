export function meterSocketUrl(locationLike) {
  const protocol = locationLike.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${locationLike.hostname}:14565/api/websocket/meter`;
}

export function parseMeterEvent(data) {
  const event = JSON.parse(data);
  if (typeof event.id !== "string" || !Number.isFinite(event.percent)) return null;
  return { id: event.id, percent: Math.max(0, Math.min(100, event.percent)) };
}

export function connectPipeweaverMeters(onLevel, options = {}) {
  const WebSocketImpl = options.WebSocketImpl || window.WebSocket;
  const locationLike = options.locationLike || window.location;
  const reconnectMs = options.reconnectMs ?? 1500;
  let socket;
  let reconnectTimer;
  let stopped = false;

  const connect = () => {
    if (stopped || document.hidden) return;
    socket = new WebSocketImpl(meterSocketUrl(locationLike));
    socket.addEventListener("message", (message) => {
      try {
        const event = parseMeterEvent(message.data);
        if (event) onLevel(event.id, event.percent);
      } catch {
        // Ignore a malformed meter frame; later frames remain usable.
      }
    });
    socket.addEventListener("close", () => {
      if (!stopped && !document.hidden) reconnectTimer = window.setTimeout(connect, reconnectMs);
    });
  };

  const visibility = () => {
    window.clearTimeout(reconnectTimer);
    if (document.hidden) socket?.close();
    else if (!socket || socket.readyState === WebSocketImpl.CLOSED) connect();
  };
  document.addEventListener("visibilitychange", visibility);
  connect();

  return () => {
    stopped = true;
    window.clearTimeout(reconnectTimer);
    document.removeEventListener("visibilitychange", visibility);
    socket?.close();
  };
}
