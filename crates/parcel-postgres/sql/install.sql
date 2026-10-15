-- parcel for Postgres: a registry of compiled contracts, and the functions that serve them.
-- Safe to run again: the schema and registry are created when missing, the functions replaced.

CREATE SCHEMA IF NOT EXISTS parcel;

CREATE TABLE IF NOT EXISTS parcel.contracts (
  name             text PRIMARY KEY,   -- <contract>@<version>
  contract         text NOT NULL,
  version          text NOT NULL,
  compilation_hash text NOT NULL,
  source           text NOT NULL,      -- the relation the query reads
  query            text NOT NULL,      -- the governed query
  artifact         jsonb NOT NULL,     -- the query's parts, rule by rule
  registered_at    timestamptz NOT NULL DEFAULT now()
);

-- Raises a refusal. A decide rule's guard calls it when the caller fails the rule.
CREATE OR REPLACE FUNCTION parcel.refuse(message text) RETURNS boolean
LANGUAGE plpgsql AS $parcel$
BEGIN
  RAISE EXCEPTION USING MESSAGE = 'parcel: ' || message, ERRCODE = 'insufficient_privilege';
END
$parcel$;

-- Sets the caller for this session. Each field is a setting the contracts' queries read:
-- parcel.id, parcel.tenant, ..., and parcel.other_<field> for each key of `other`.
CREATE OR REPLACE FUNCTION parcel.caller(
  id text,
  tenant text,
  purpose text,
  tier text DEFAULT NULL,
  clearance bigint DEFAULT NULL,
  classification text DEFAULT NULL,
  roles text[] DEFAULT NULL,
  now timestamptz DEFAULT NULL,
  other jsonb DEFAULT NULL
) RETURNS void
LANGUAGE plpgsql AS $parcel$
DECLARE
  k text;
  v text;
BEGIN
  PERFORM set_config('parcel.id', coalesce(id, ''), false);
  PERFORM set_config('parcel.tenant', coalesce(tenant, ''), false);
  PERFORM set_config('parcel.purpose', coalesce(purpose, ''), false);
  PERFORM set_config('parcel.tier', coalesce(tier, ''), false);
  PERFORM set_config('parcel.clearance', coalesce(clearance::text, ''), false);
  PERFORM set_config('parcel.classification', coalesce(classification, ''), false);
  PERFORM set_config('parcel.roles', coalesce(array_to_string(roles, ','), ''), false);
  PERFORM set_config('parcel.now', coalesce(now::text, ''), false);
  FOR k, v IN SELECT key, value FROM jsonb_each_text(coalesce(other, '{}'::jsonb)) LOOP
    PERFORM set_config('parcel.other_' || k, coalesce(v, ''), false);
  END LOOP;
END
$parcel$;

-- Makes contract `contract_name` queryable in this session, as a temporary view of the same
-- name. The view reads the caller's settings each time it is queried.
CREATE OR REPLACE FUNCTION parcel.use(contract_name text) RETURNS text
LANGUAGE plpgsql AS $parcel$
DECLARE
  q text;
BEGIN
  SELECT query INTO q FROM parcel.contracts WHERE name = contract_name;
  IF q IS NULL THEN
    RAISE EXCEPTION 'parcel: no contract named %', contract_name
      USING HINT = 'SELECT name FROM parcel.contracts lists them';
  END IF;
  IF to_regclass(format('pg_temp.%I', contract_name)) IS NOT NULL THEN
    EXECUTE format('DROP VIEW pg_temp.%I', contract_name);
  END IF;
  EXECUTE format('CREATE TEMP VIEW %I AS %s', contract_name, q);
  RETURN contract_name;
END
$parcel$;

-- Makes every registered contract queryable in this session.
CREATE OR REPLACE FUNCTION parcel.use_all() RETURNS SETOF text
LANGUAGE sql AS $parcel$
  SELECT parcel.use(name) FROM parcel.contracts ORDER BY name
$parcel$;
