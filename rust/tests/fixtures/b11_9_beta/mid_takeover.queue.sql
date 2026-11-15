PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE "solid_queue_jobs" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "active_job_id" varchar, "arguments" text, "class_name" varchar NOT NULL, "concurrency_key" varchar, "created_at" datetime(6) NOT NULL, "finished_at" datetime(6), "priority" integer DEFAULT 0 NOT NULL, "queue_name" varchar NOT NULL, "scheduled_at" datetime(6), "updated_at" datetime(6) NOT NULL);
INSERT INTO "solid_queue_jobs" (rowid, "id", "active_job_id", "arguments", "class_name", "concurrency_key", "created_at", "finished_at", "priority", "queue_name", "scheduled_at", "updated_at") VALUES (1, 1, 'job-Tracker::LedgerJob', '{"job_class":"Tracker::LedgerJob","arguments":[1]}', 'Tracker::LedgerJob', NULL, '2026-09-30 11:00:00', NULL, 0, 'default', NULL, '2026-09-30 11:00:00');
INSERT INTO "solid_queue_jobs" (rowid, "id", "active_job_id", "arguments", "class_name", "concurrency_key", "created_at", "finished_at", "priority", "queue_name", "scheduled_at", "updated_at") VALUES (2, 2, 'job-Tracker::LedgerJob::Retry', '{"job_class":"Tracker::LedgerJob::Retry","arguments":[1]}', 'Tracker::LedgerJob::Retry', NULL, '2026-09-30 11:00:00', NULL, 0, 'default', NULL, '2026-09-30 11:00:00');
INSERT INTO "solid_queue_jobs" (rowid, "id", "active_job_id", "arguments", "class_name", "concurrency_key", "created_at", "finished_at", "priority", "queue_name", "scheduled_at", "updated_at") VALUES (3, 3, 'job-PortfolioSnapshot::BackfillJob', '{"job_class":"PortfolioSnapshot::BackfillJob","arguments":[1,null]}', 'PortfolioSnapshot::BackfillJob', NULL, '2026-09-30 11:00:00', NULL, 0, 'default', NULL, '2026-09-30 11:00:00');
INSERT INTO "solid_queue_jobs" (rowid, "id", "active_job_id", "arguments", "class_name", "concurrency_key", "created_at", "finished_at", "priority", "queue_name", "scheduled_at", "updated_at") VALUES (4, 4, '00000000-0000-4000-8000-000000000001', '{"job_class":"Bot::SignalJob","job_id":"00000000-0000-4000-8000-000000000001","provider_job_id":null,"queue_name":"kraken","priority":null,"arguments":[{"_aj_globalid":"gid://deltabadger/Bots::Signal/1"},{"_aj_globalid":"gid://deltabadger/BotSignal/1"},{"_aj_serialized":"ActiveJob::Serializers::TimeWithZoneSerializer","value":"2026-09-30T11:58:01.123456000Z","time_zone":"Etc/UTC"}],"executions":0,"exception_executions":{},"locale":"en","timezone":"UTC","enqueued_at":"2026-09-30T11:58:01.123456000Z","scheduled_at":"2026-09-30T11:58:01.123456000Z"}', 'Bot::SignalJob', 'Bot::ActionJob/exchange_kraken', '2026-09-30 11:58:01.123456', NULL, 0, 'kraken', '2026-09-30 11:58:01.123456', '2026-09-30 11:58:01.123456');
INSERT INTO "solid_queue_jobs" (rowid, "id", "active_job_id", "arguments", "class_name", "concurrency_key", "created_at", "finished_at", "priority", "queue_name", "scheduled_at", "updated_at") VALUES (5, 5, '00000000-0000-4000-8000-000000000002', '{"job_class":"Bot::SignalJob","job_id":"00000000-0000-4000-8000-000000000002","provider_job_id":null,"queue_name":"kraken","priority":null,"arguments":[{"_aj_globalid":"gid://deltabadger/Bots::Signal/1"},{"_aj_globalid":"gid://deltabadger/BotSignal/2"},{"_aj_serialized":"ActiveJob::Serializers::TimeWithZoneSerializer","value":"2026-09-30T11:58:02.123456000Z","time_zone":"Etc/UTC"}],"executions":0,"exception_executions":{},"locale":"en","timezone":"UTC","enqueued_at":"2026-09-30T11:58:02.123456000Z","scheduled_at":"2026-09-30T11:58:02.123456000Z"}', 'Bot::SignalJob', 'Bot::ActionJob/exchange_kraken', '2026-09-30 11:58:02.123456', NULL, 0, 'kraken', '2026-09-30 11:58:02.123456', '2026-09-30 11:58:02.123456');
INSERT INTO "solid_queue_jobs" (rowid, "id", "active_job_id", "arguments", "class_name", "concurrency_key", "created_at", "finished_at", "priority", "queue_name", "scheduled_at", "updated_at") VALUES (6, 6, 'job-Rule::ExecuteJob', '{"job_class":"Rule::ExecuteJob","arguments":[{"_aj_globalid":"gid://deltabadger/Rules::Withdrawal/1"}]}', 'Rule::ExecuteJob', NULL, '2026-09-30 11:00:00', NULL, 0, 'default', NULL, '2026-09-30 11:00:00');
INSERT INTO "solid_queue_jobs" (rowid, "id", "active_job_id", "arguments", "class_name", "concurrency_key", "created_at", "finished_at", "priority", "queue_name", "scheduled_at", "updated_at") VALUES (7, 7, 'job-Rule::EvaluateAllJob', '{"job_class":"Rule::EvaluateAllJob","arguments":[]}', 'Rule::EvaluateAllJob', NULL, '2026-09-30 11:00:00', NULL, 0, 'default', NULL, '2026-09-30 11:00:00');
CREATE TABLE "solid_queue_pauses" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "queue_name" varchar NOT NULL);
CREATE TABLE "solid_queue_processes" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "hostname" varchar, "kind" varchar NOT NULL, "last_heartbeat_at" datetime(6) NOT NULL, "metadata" text, "name" varchar NOT NULL, "pid" integer NOT NULL, "supervisor_id" bigint);
CREATE TABLE "solid_queue_recurring_tasks" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "arguments" text, "class_name" varchar, "command" varchar(2048), "created_at" datetime(6) NOT NULL, "description" text, "key" varchar NOT NULL, "priority" integer DEFAULT 0, "queue_name" varchar, "schedule" varchar NOT NULL, "static" boolean DEFAULT TRUE NOT NULL, "updated_at" datetime(6) NOT NULL);
CREATE TABLE "solid_queue_semaphores" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "expires_at" datetime(6) NOT NULL, "key" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, "value" integer DEFAULT 1 NOT NULL);
CREATE TABLE "solid_queue_blocked_executions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "concurrency_key" varchar NOT NULL, "created_at" datetime(6) NOT NULL, "expires_at" datetime(6) NOT NULL, "job_id" bigint NOT NULL, "priority" integer DEFAULT 0 NOT NULL, "queue_name" varchar NOT NULL, CONSTRAINT "fk_rails_4cd34e2228"
FOREIGN KEY ("job_id")
  REFERENCES "solid_queue_jobs" ("id")
 ON DELETE CASCADE);
CREATE TABLE "solid_queue_claimed_executions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "job_id" bigint NOT NULL, "process_id" bigint, CONSTRAINT "fk_rails_9cfe4d4944"
FOREIGN KEY ("job_id")
  REFERENCES "solid_queue_jobs" ("id")
 ON DELETE CASCADE);
INSERT INTO "solid_queue_claimed_executions" (rowid, "id", "created_at", "job_id", "process_id") VALUES (1, 1, '2026-09-30 11:00:00', 3, 7);
INSERT INTO "solid_queue_claimed_executions" (rowid, "id", "created_at", "job_id", "process_id") VALUES (2, 2, '2026-09-30 11:00:00', 5, 7);
CREATE TABLE "solid_queue_failed_executions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "error" text, "job_id" bigint NOT NULL, CONSTRAINT "fk_rails_39bbc7a631"
FOREIGN KEY ("job_id")
  REFERENCES "solid_queue_jobs" ("id")
 ON DELETE CASCADE);
INSERT INTO "solid_queue_failed_executions" (rowid, "id", "created_at", "error", "job_id") VALUES (1, 1, '2026-09-30 11:00:00', 'pruned', 2);
INSERT INTO "solid_queue_failed_executions" (rowid, "id", "created_at", "error", "job_id") VALUES (2, 2, '2026-09-30 11:00:00', 'pruned', 7);
CREATE TABLE "solid_queue_ready_executions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "job_id" bigint NOT NULL, "priority" integer DEFAULT 0 NOT NULL, "queue_name" varchar NOT NULL, CONSTRAINT "fk_rails_81fcbd66af"
FOREIGN KEY ("job_id")
  REFERENCES "solid_queue_jobs" ("id")
 ON DELETE CASCADE);
INSERT INTO "solid_queue_ready_executions" (rowid, "id", "created_at", "job_id", "priority", "queue_name") VALUES (1, 1, '2026-09-30 11:00:00', 1, 0, 'default');
INSERT INTO "solid_queue_ready_executions" (rowid, "id", "created_at", "job_id", "priority", "queue_name") VALUES (2, 2, '2026-09-30 11:58:01.123456', 4, 0, 'kraken');
INSERT INTO "solid_queue_ready_executions" (rowid, "id", "created_at", "job_id", "priority", "queue_name") VALUES (3, 3, '2026-09-30 11:00:00', 6, 0, 'default');
CREATE TABLE "solid_queue_recurring_executions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "job_id" bigint NOT NULL, "run_at" datetime(6) NOT NULL, "task_key" varchar NOT NULL, CONSTRAINT "fk_rails_318a5533ed"
FOREIGN KEY ("job_id")
  REFERENCES "solid_queue_jobs" ("id")
 ON DELETE CASCADE);
CREATE TABLE "solid_queue_scheduled_executions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "job_id" bigint NOT NULL, "priority" integer DEFAULT 0 NOT NULL, "queue_name" varchar NOT NULL, "scheduled_at" datetime(6) NOT NULL, CONSTRAINT "fk_rails_c4316f352d"
FOREIGN KEY ("job_id")
  REFERENCES "solid_queue_jobs" ("id")
 ON DELETE CASCADE);
CREATE TABLE "schema_migrations" ("version" varchar NOT NULL PRIMARY KEY);
INSERT INTO "schema_migrations" (rowid, "version") VALUES (1, '1');
CREATE TABLE "ar_internal_metadata" ("key" varchar NOT NULL PRIMARY KEY, "value" varchar, "created_at" datetime(6) NOT NULL, "updated_at" datetime(6) NOT NULL);
INSERT INTO "ar_internal_metadata" (rowid, "key", "value", "created_at", "updated_at") VALUES (1, 'environment', 'development', '2026-11-15 17:37:56.140224', '2026-11-15 17:37:56.140224');
DELETE FROM sqlite_sequence;
INSERT INTO sqlite_sequence (name, seq) VALUES ('solid_queue_blocked_executions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('solid_queue_claimed_executions', 2);
INSERT INTO sqlite_sequence (name, seq) VALUES ('solid_queue_failed_executions', 2);
INSERT INTO sqlite_sequence (name, seq) VALUES ('solid_queue_ready_executions', 3);
INSERT INTO sqlite_sequence (name, seq) VALUES ('solid_queue_recurring_executions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('solid_queue_scheduled_executions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('solid_queue_jobs', 7);
CREATE INDEX "index_solid_queue_jobs_on_active_job_id" ON "solid_queue_jobs" ("active_job_id");
CREATE INDEX "index_solid_queue_jobs_on_class_name" ON "solid_queue_jobs" ("class_name");
CREATE INDEX "index_solid_queue_jobs_on_finished_at" ON "solid_queue_jobs" ("finished_at");
CREATE INDEX "index_solid_queue_jobs_for_filtering" ON "solid_queue_jobs" ("queue_name", "finished_at");
CREATE INDEX "index_solid_queue_jobs_for_alerting" ON "solid_queue_jobs" ("scheduled_at", "finished_at");
CREATE UNIQUE INDEX "index_solid_queue_pauses_on_queue_name" ON "solid_queue_pauses" ("queue_name");
CREATE INDEX "index_solid_queue_processes_on_last_heartbeat_at" ON "solid_queue_processes" ("last_heartbeat_at");
CREATE UNIQUE INDEX "index_solid_queue_processes_on_name_and_supervisor_id" ON "solid_queue_processes" ("name", "supervisor_id");
CREATE INDEX "index_solid_queue_processes_on_supervisor_id" ON "solid_queue_processes" ("supervisor_id");
CREATE UNIQUE INDEX "index_solid_queue_recurring_tasks_on_key" ON "solid_queue_recurring_tasks" ("key");
CREATE INDEX "index_solid_queue_recurring_tasks_on_static" ON "solid_queue_recurring_tasks" ("static");
CREATE INDEX "index_solid_queue_semaphores_on_expires_at" ON "solid_queue_semaphores" ("expires_at");
CREATE INDEX "index_solid_queue_semaphores_on_key_and_value" ON "solid_queue_semaphores" ("key", "value");
CREATE UNIQUE INDEX "index_solid_queue_semaphores_on_key" ON "solid_queue_semaphores" ("key");
CREATE INDEX "index_solid_queue_blocked_executions_for_release" ON "solid_queue_blocked_executions" ("concurrency_key", "priority", "job_id");
CREATE INDEX "index_solid_queue_blocked_executions_for_maintenance" ON "solid_queue_blocked_executions" ("expires_at", "concurrency_key");
CREATE UNIQUE INDEX "index_solid_queue_blocked_executions_on_job_id" ON "solid_queue_blocked_executions" ("job_id");
CREATE UNIQUE INDEX "index_solid_queue_claimed_executions_on_job_id" ON "solid_queue_claimed_executions" ("job_id");
CREATE INDEX "index_solid_queue_claimed_executions_on_process_id_and_job_id" ON "solid_queue_claimed_executions" ("process_id", "job_id");
CREATE UNIQUE INDEX "index_solid_queue_failed_executions_on_job_id" ON "solid_queue_failed_executions" ("job_id");
CREATE UNIQUE INDEX "index_solid_queue_ready_executions_on_job_id" ON "solid_queue_ready_executions" ("job_id");
CREATE INDEX "index_solid_queue_poll_all" ON "solid_queue_ready_executions" ("priority", "job_id");
CREATE INDEX "index_solid_queue_poll_by_queue" ON "solid_queue_ready_executions" ("queue_name", "priority", "job_id");
CREATE UNIQUE INDEX "index_solid_queue_recurring_executions_on_job_id" ON "solid_queue_recurring_executions" ("job_id");
CREATE UNIQUE INDEX "index_solid_queue_recurring_executions_on_task_key_and_run_at" ON "solid_queue_recurring_executions" ("task_key", "run_at");
CREATE UNIQUE INDEX "index_solid_queue_scheduled_executions_on_job_id" ON "solid_queue_scheduled_executions" ("job_id");
CREATE INDEX "index_solid_queue_dispatch_all" ON "solid_queue_scheduled_executions" ("scheduled_at", "priority", "job_id");
COMMIT;
