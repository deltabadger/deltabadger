CREATE TABLE "solid_cache_entries" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "byte_size" integer(4) NOT NULL, "created_at" datetime(6) NOT NULL, "key" blob(1024) NOT NULL, "key_hash" integer(8) NOT NULL, "value" blob(536870912) NOT NULL);
CREATE INDEX "index_solid_cache_entries_on_byte_size" ON "solid_cache_entries" ("byte_size");
CREATE INDEX "index_solid_cache_entries_on_key_hash_and_byte_size" ON "solid_cache_entries" ("key_hash", "byte_size");
CREATE UNIQUE INDEX "index_solid_cache_entries_on_key_hash" ON "solid_cache_entries" ("key_hash");
CREATE TABLE "schema_migrations" ("version" varchar NOT NULL PRIMARY KEY);
CREATE TABLE "ar_internal_metadata" ("key" varchar NOT NULL PRIMARY KEY, "value" varchar, "created_at" datetime(6) NOT NULL, "updated_at" datetime(6) NOT NULL);
INSERT INTO "schema_migrations" ("version") VALUES ('1');
INSERT INTO "ar_internal_metadata" ("key", "value", "created_at", "updated_at") VALUES ('environment', 'production', '2026-01-01 00:00:00', '2026-01-01 00:00:00');
