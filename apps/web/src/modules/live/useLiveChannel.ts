import { useEffect, useRef, useState } from "react";

import { subscribe, type Hint, type LiveStatus } from "@/services/realtime";

/**
 * Keep a realtime subscription open while mounted. `channel` undefined = not yet known.
 * The handlers may change between renders without reconnecting.
 */
export function useLiveChannel(channel: string | undefined, onHint: (hint: Hint) => void, onResync: () => void): LiveStatus {
  const [status, setStatus] = useState<LiveStatus>("connecting");
  const handlers = useRef({ onHint, onResync });
  handlers.current = { onHint, onResync };

  useEffect(() => {
    if (!channel) return undefined;
    return subscribe(channel, {
      onHint: (hint) => handlers.current.onHint(hint),
      onResync: () => handlers.current.onResync(),
      onStatus: setStatus,
    });
  }, [channel]);

  return status;
}
