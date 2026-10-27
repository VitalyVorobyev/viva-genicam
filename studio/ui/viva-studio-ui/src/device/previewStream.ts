/** One preview socket per endpoint; hidden canvases do not decode its frames. */
type PreviewData = string | ArrayBuffer;

interface PreviewListener {
  message: (data: PreviewData) => void;
  error: () => void;
  close: () => void;
}

interface PreviewConnection {
  socket: WebSocket;
  listeners: Set<PreviewListener>;
  info: string | null;
  frame: ArrayBuffer | null;
}

const connections = new Map<string, PreviewConnection>();

/**
 * Share the existing preview transport between the Image tab and the optional
 * configuration canvas. Retain only the latest bytes, without copying/decoding,
 * so a late viewer can also display a stopped SingleFrame acquisition.
 */
export function followPreview(wsUrl: string, listener: PreviewListener, streamKey = wsUrl): () => void {
  const key = `${streamKey}\0${wsUrl}`;
  let connection = connections.get(key);
  if (!connection) {
    const socket = new WebSocket(wsUrl);
    socket.binaryType = "arraybuffer";
    connection = { socket, listeners: new Set(), info: null, frame: null };
    connections.set(key, connection);
    const current = connection;
    socket.onopen = () => { console.info("[PreviewStream] WebSocket opened", { wsUrl }); };
    socket.onmessage = (event) => {
      const data: unknown = event.data;
      if (typeof data === "string") current.info = data;
      else if (data instanceof ArrayBuffer) current.frame = data;
      else return;
      for (const subscriber of current.listeners) subscriber.message(data);
    };
    socket.onerror = (event) => {
      console.error("[PreviewStream] WebSocket error", { wsUrl, event });
      for (const subscriber of current.listeners) subscriber.error();
    };
    socket.onclose = (event) => {
      console.info("[PreviewStream] WebSocket closed", { wsUrl, code: event?.code, reason: event?.reason, wasClean: event?.wasClean });
      if (connections.get(key) === current) connections.delete(key);
      current.info = null;
      current.frame = null;
      for (const subscriber of current.listeners) subscriber.close();
    };
  }
  connection.listeners.add(listener);
  if (connection.info !== null) listener.message(connection.info);
  if (connection.frame !== null) listener.message(connection.frame);

  const current = connection;
  let following = true;
  return () => {
    if (!following) return;
    following = false;
    current.listeners.delete(listener);
    if (current.listeners.size !== 0) return;
    if (connections.get(key) === current) connections.delete(key);
    current.socket.onopen = null;
    current.socket.onmessage = null;
    current.socket.onerror = null;
    current.socket.onclose = null;
    current.socket.close();
  };
}
