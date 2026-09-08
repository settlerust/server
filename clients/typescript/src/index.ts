export type * from './types.ts';
import type { ApiError, Command, CommandRequest, CreateGame, EventPage, GameList, GameView, Receipt, ServerFrame, TokenResponse } from './types.ts';

export class ServerError extends Error {
  readonly data: ApiError;
  constructor(data: ApiError) { super(data.message); this.data = data; }
}
export class SettlersClient {
  readonly base: string;
  token: string;
  constructor(base: string, token = '') { this.base = base; this.token = token; }
  static async guest(base: string, name: string): Promise<SettlersClient> {
    const client = new SettlersClient(base.replace(/\/$/, ''));
    const response = await client.request<TokenResponse>('/v1/auth/guest', { name });
    client.token = response.token;
    return client;
  }
  async request<T>(path: string, body?: unknown): Promise<T> {
    const response = await fetch(this.base + path, { method: body === undefined ? 'GET' : 'POST',
      headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(30_000) });
    const value: unknown = await response.json();
    if (!response.ok) throw new ServerError(value as ApiError);
    return value as T;
  }
  async refresh(): Promise<TokenResponse> { const value = await this.request<TokenResponse>('/v1/auth/refresh', null); this.token = value.token; return value; }
  create(request: CreateGame): Promise<GameView> { return this.request('/v1/games', request); }
  game(id: string): Promise<GameView> { return this.request(`/v1/games/${encodeURIComponent(id)}`); }
  games(): Promise<GameList> { return this.request('/v1/games'); }
  prepare(version: number, command: Command): CommandRequest { return { request_id: crypto.randomUUID(), expected_version: version, command }; }
  submit(id: string, request: CommandRequest): Promise<Receipt> { return this.request(`/v1/games/${encodeURIComponent(id)}/commands`, request); }
  history(id: string, after = 0, limit = 100): Promise<EventPage> { return this.request(`/v1/games/${encodeURIComponent(id)}/events?after=${after}&limit=${limit}`); }
  /** Reconnects with the same identity. Replay missing events through history using a separate event cursor. */
  connect(id: string, onFrame: (frame: ServerFrame) => void, onError: (error: unknown) => void = console.error): () => void {
    let stopped = false; let socket: WebSocket | undefined; let delay = 250; let timer: ReturnType<typeof setTimeout> | undefined;
    const open = () => {
      if (stopped) return;
      socket = new WebSocket(this.base.replace(/^http/, 'ws') + `/v1/games/${encodeURIComponent(id)}/ws`, ['settlers-v1', `bearer.${this.token}`]);
      socket.onopen = () => { delay = 250; };
      socket.onmessage = event => { try { onFrame(JSON.parse(String(event.data)) as ServerFrame); } catch (error) { onError(error); } };
      socket.onerror = onError;
      socket.onclose = () => { if (!stopped) { timer = setTimeout(open, delay); delay = Math.min(15_000, delay * 2); } };
    };
    open();
    return () => { stopped = true; if (timer) clearTimeout(timer); socket?.close(); };
  }
}
