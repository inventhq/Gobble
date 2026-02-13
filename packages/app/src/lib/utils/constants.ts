export const API_URL = import.meta.env.VITE_API_URL || 'http://localhost:8787';

export const EVENT_TYPES = ['click', 'postback', 'impression'] as const;
export type EventType = (typeof EVENT_TYPES)[number];

export const PLANS = ['free', 'pro', 'enterprise'] as const;
export type Plan = (typeof PLANS)[number];

export const ROLES = ['admin', 'tenant'] as const;
export type Role = (typeof ROLES)[number];
