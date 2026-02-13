import { api } from './client';

export interface TrackingEvent {
  event_id: string;
  event_type: string;
  timestamp: number;
  ip: string;
  user_agent: string;
  referer: string | null;
  request_path: string;
  request_host: string;
  params: Record<string, string>;
  raw_payload?: Record<string, any> | null;
}

export interface EventsResponse {
  events: TrackingEvent[];
  count: number;
  limit: number;
  offset: number;
  server_time: number;
}

export interface HourlyStat {
  event_type: string;
  hour: number;
  count: number;
}

export interface StatsSummary {
  event_type: string;
  total: number;
}

export interface StatsResponse {
  hourly: HourlyStat[];
  summary: StatsSummary[];
  hours: number;
  from_hour: number;
  to_hour: number;
  server_time: number;
  filters?: { param_key: string; param_value: string | null };
}

export interface BreakdownItem {
  param_value: string;
  event_type: string;
  total: number;
}

export interface BreakdownResponse {
  breakdown: BreakdownItem[];
  group_by: string;
  hours: number;
  from_hour: number;
  to_hour: number;
  server_time: number;
}

export async function getEvents(params?: {
  limit?: number;
  offset?: number;
  event_type?: string;
  since?: number;
  tu_id?: string;
  param_key?: string;
  param_value?: string;
}): Promise<EventsResponse> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set('limit', String(params.limit));
  if (params?.offset) qs.set('offset', String(params.offset));
  if (params?.event_type) qs.set('event_type', params.event_type);
  if (params?.since) qs.set('since', String(params.since));
  if (params?.tu_id) qs.set('tu_id', params.tu_id);
  if (params?.param_key) qs.set('param_key', params.param_key);
  if (params?.param_value) qs.set('param_value', params.param_value);
  const query = qs.toString();
  return api<EventsResponse>('GET', `/api/events${query ? `?${query}` : ''}`);
}

export async function getStats(params?: {
  hours?: number;
  event_type?: string;
  tu_id?: string;
  param_key?: string;
  param_value?: string;
  group_by?: string;
}): Promise<StatsResponse> {
  const qs = new URLSearchParams();
  if (params?.hours) qs.set('hours', String(params.hours));
  if (params?.event_type) qs.set('event_type', params.event_type);
  if (params?.tu_id) qs.set('tu_id', params.tu_id);
  if (params?.param_key) qs.set('param_key', params.param_key);
  if (params?.param_value) qs.set('param_value', params.param_value);
  if (params?.group_by) qs.set('group_by', params.group_by);
  const query = qs.toString();
  return api<StatsResponse>('GET', `/api/events/stats${query ? `?${query}` : ''}`);
}

export interface MatchTriggerEvent {
  event_id: string;
  timestamp: number;
  ip: string;
  user_agent: string;
  params: Record<string, string>;
}

export interface MatchResultEvent {
  event_id: string;
  timestamp: number;
  params: Record<string, string>;
}

export interface MatchPair {
  on: string;
  on_value: string;
  trigger_event: MatchTriggerEvent;
  result_event: MatchResultEvent | null;
  time_delta_ms: number | null;
  matched: boolean;
}

export interface MatchSummary {
  total_triggers: number;
  matched: number;
  unmatched: number;
  match_rate: number;
}

export interface MatchResponse {
  pairs: MatchPair[];
  summary: MatchSummary;
  trigger: string;
  result: string;
  on: string;
  hours: number;
  limit: number;
  offset: number;
  server_time: number;
}

export async function getMatches(params?: {
  trigger?: string;
  result?: string;
  on?: string;
  tu_id?: string;
  hours?: number;
  limit?: number;
  offset?: number;
  param_key?: string;
  param_value?: string;
}): Promise<MatchResponse> {
  const qs = new URLSearchParams();
  if (params?.trigger) qs.set('trigger', params.trigger);
  if (params?.result) qs.set('result', params.result);
  if (params?.on) qs.set('on', params.on);
  if (params?.tu_id) qs.set('tu_id', params.tu_id);
  if (params?.hours) qs.set('hours', String(params.hours));
  if (params?.limit) qs.set('limit', String(params.limit));
  if (params?.offset) qs.set('offset', String(params.offset));
  if (params?.param_key) qs.set('param_key', params.param_key);
  if (params?.param_value) qs.set('param_value', params.param_value);
  const query = qs.toString();
  return api<MatchResponse>('GET', `/api/events/match${query ? `?${query}` : ''}`);
}

// --- Historical / Cold Storage Queries ---

export interface HistoryRow {
  [key: string]: unknown;
}

export interface HistoryResponse {
  count: number;
  rows: HistoryRow[];
  partitions_scanned: number;
  query_ms: number;
  source: string;
  server_time: number;
}

export interface MergedStatsResponse {
  summary: StatsSummary[];
  hourly: HourlyStat[];
  hours: number;
  hot_hours: number;
  cold_range: { from: string; to: string } | null;
  sources: {
    hot: boolean;
    cold: boolean;
    hot_partitions: string | null;
    cold_partitions: number;
    cold_query_ms: number;
  };
  server_time: number;
}

export async function getHistory(params: {
  date_from: string;
  date_to: string;
  mode?: string;
  event_type?: string;
  tu_id?: string;
  group_by?: string;
  param_key?: string;
  param_value?: string;
  limit?: number;
}): Promise<HistoryResponse> {
  const qs = new URLSearchParams();
  qs.set('date_from', params.date_from);
  qs.set('date_to', params.date_to);
  if (params.mode) qs.set('mode', params.mode);
  if (params.event_type) qs.set('event_type', params.event_type);
  if (params.tu_id) qs.set('tu_id', params.tu_id);
  if (params.group_by) qs.set('group_by', params.group_by);
  if (params.param_key) qs.set('param_key', params.param_key);
  if (params.param_value) qs.set('param_value', params.param_value);
  if (params.limit) qs.set('limit', String(params.limit));
  return api<HistoryResponse>('GET', `/api/events/history?${qs.toString()}`);
}

export async function getMergedStats(params?: {
  hours?: number;
  date_from?: string;
  date_to?: string;
  event_type?: string;
  tu_id?: string;
  group_by?: string;
}): Promise<MergedStatsResponse> {
  const qs = new URLSearchParams();
  if (params?.hours) qs.set('hours', String(params.hours));
  if (params?.date_from) qs.set('date_from', params.date_from);
  if (params?.date_to) qs.set('date_to', params.date_to);
  if (params?.event_type) qs.set('event_type', params.event_type);
  if (params?.tu_id) qs.set('tu_id', params.tu_id);
  if (params?.group_by) qs.set('group_by', params.group_by);
  const query = qs.toString();
  return api<MergedStatsResponse>('GET', `/api/events/stats/merged${query ? `?${query}` : ''}`);
}

export async function getBreakdown(params: {
  group_by: string;
  hours?: number;
  event_type?: string;
  tu_id?: string;
  param_key?: string;
  param_value?: string;
}): Promise<BreakdownResponse> {
  const qs = new URLSearchParams();
  qs.set('group_by', params.group_by);
  if (params.hours) qs.set('hours', String(params.hours));
  if (params.event_type) qs.set('event_type', params.event_type);
  if (params.tu_id) qs.set('tu_id', params.tu_id);
  if (params.param_key) qs.set('param_key', params.param_key);
  if (params.param_value) qs.set('param_value', params.param_value);
  return api<BreakdownResponse>('GET', `/api/events/stats?${qs.toString()}`);
}
