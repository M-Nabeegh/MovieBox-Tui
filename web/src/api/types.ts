export type ErrorEnvelope = { error: { code: string; message: string; request_id: string; fields: Record<string, string> } };
export type Session = { authenticated: boolean; username: string; csrf_token: string };
export type ApiRequestInit = RequestInit & { retryOnAuth?: boolean };
