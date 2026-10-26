import type { Contributions } from './contributions.ts';
import type { IssueReports } from './issue-reports.ts';
import type { ShareRoom, TurnBudget } from './worker.ts';
export interface Env {
  ISSUE_REPORTS: R2Bucket;
  REPORTS_INDEX: D1Database;
  CONTRIBUTIONS: DurableObjectNamespace<Contributions>;
  CONTRIBUTION_LIMIT: RateLimit;
  ISSUE_LIMIT: RateLimit;
  ERROR_LIMIT: RateLimit;
  CONNECTIONS: RateLimit;
  ALLOWED_ORIGINS?: string;
  TURN_TTL_SECONDS?: string;
  TURN_DAILY_ISSUE_LIMIT?: string;
  TURN_KEY_ID?: string;
  TURN_API_TOKEN?: string;
  GITHUB_ISSUES_TOKEN?: string;
  ISSUES: DurableObjectNamespace<IssueReports>;
  ROOMS: DurableObjectNamespace<ShareRoom>;
  TURN_BUDGET: DurableObjectNamespace<TurnBudget>;
}
export interface BudgetRecord {
  day: string;
  count: number;
}
export interface IceServer {
  urls: string | string[];
  username?: string;
  credential?: string;
}
export interface Attachment {
  role: 'pending' | 'preparing' | 'host' | 'client' | 'closed';
  room: string | null;
  accepted: number;
  window?: number;
  messages?: number;
  peer?: string;
  expires?: number;
  iceServers?: IceServer[];
}
