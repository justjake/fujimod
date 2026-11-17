CREATE TABLE `download_event` (
	`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`created_at` integer NOT NULL,
	`release` text NOT NULL,
	`kind` text NOT NULL,
	`firmware` text,
	`cameras` integer NOT NULL,
	`session` text
);
--> statement-breakpoint
CREATE INDEX `downloadEvent_createdAt_idx` ON `download_event` (`created_at`);--> statement-breakpoint
CREATE INDEX `downloadEvent_release_idx` ON `download_event` (`release`);--> statement-breakpoint
CREATE INDEX `downloadEvent_session_idx` ON `download_event` (`session`);