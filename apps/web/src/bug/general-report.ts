export const MAX_REPORT_ARCHIVE_BYTES = 1024 * 1024 * 1024;
export const CONTACT_TYPES = ['Instagram', 'Twitter', 'Email'] as const;
export type ContactType = (typeof CONTACT_TYPES)[number];
