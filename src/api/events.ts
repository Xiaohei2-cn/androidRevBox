import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * 事件订阅统一出口（与 api/client.ts 收口 invoke 同理）：
 * payload 外层为后端 core::ipc::AppEvent 信封 { event, timestamp, payload }。
 */
export interface EventEnvelope<P> {
  event: string;
  timestamp: number;
  payload: P;
}

export async function listenEvent<P>(
  name: string,
  handler: (payload: P, envelope: EventEnvelope<P>) => void,
): Promise<UnlistenFn> {
  return listen<EventEnvelope<P>>(name, (e) => {
    handler(e.payload.payload, e.payload);
  });
}
