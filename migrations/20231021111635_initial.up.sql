-- generated via: 
-- `pg_dump --schema-only --no-owner cratesfyi`
--
-- and then manually removing 
-- * the `public.` schema references from this file, so it also works for the test setup
-- * the `SET` settings

CREATE SCHEMA IF NOT EXISTS public;

CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;
COMMENT ON EXTENSION fuzzystrmatch IS 'determine similarities and distance between strings';



CREATE TYPE feature AS (
	name text,
	subfeatures text[]
);



CREATE FUNCTION normalize_crate_name(character varying) RETURNS character varying
    LANGUAGE sql
    AS $_$
                    SELECT LOWER(REPLACE($1, '_', '-'));
                $_$;




CREATE TABLE blacklisted_crates (
    crate_name character varying NOT NULL
);



CREATE TABLE builds (
    id integer NOT NULL,
    rid integer NOT NULL,
    rustc_version character varying(100) NOT NULL,
    docsrs_version character varying(100) NOT NULL,
    build_status boolean NOT NULL,
    build_time timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    output text,
    build_server text DEFAULT ''::text NOT NULL
);



CREATE SEQUENCE builds_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE builds_id_seq OWNED BY builds.id;



CREATE TABLE cdn_invalidation_queue (
    id bigint NOT NULL,
    crate character varying(255) NOT NULL,
    cdn_distribution_id character varying(255) NOT NULL,
    path_pattern text NOT NULL,
    queued timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    created_in_cdn timestamp with time zone,
    cdn_reference character varying(255)
);



CREATE SEQUENCE cdn_invalidation_queue_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE cdn_invalidation_queue_id_seq OWNED BY cdn_invalidation_queue.id;



CREATE TABLE compression_rels (
    release integer NOT NULL,
    algorithm integer
);



CREATE TABLE config (
    name character varying(100) NOT NULL,
    value json NOT NULL
);



CREATE TABLE crate_priorities (
    pattern character varying NOT NULL,
    priority integer NOT NULL
);



CREATE TABLE crates (
    id integer NOT NULL,
    name character varying(255) NOT NULL,
    latest_version_id integer DEFAULT 0,
    downloads_total integer DEFAULT 0
);



CREATE SEQUENCE crates_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE crates_id_seq OWNED BY crates.id;




CREATE TABLE doc_coverage (
    release_id integer NOT NULL,
    total_items integer,
    documented_items integer,
    total_items_needing_examples integer,
    items_with_examples integer
);



CREATE TABLE files (
    path character varying(4096) NOT NULL,
    mime character varying(100) NOT NULL,
    date_updated timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    content bytea,
    compression integer,
    public boolean DEFAULT false NOT NULL
);



CREATE TABLE keyword_rels (
    rid integer,
    kid integer
);



CREATE TABLE keywords (
    id integer NOT NULL,
    name character varying(255),
    slug character varying(255) NOT NULL
);



CREATE SEQUENCE keywords_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE keywords_id_seq OWNED BY keywords.id;



CREATE TABLE owner_rels (
    cid integer,
    oid integer
);



CREATE TABLE owners (
    id integer NOT NULL,
    login character varying(255) NOT NULL,
    avatar character varying(255) NOT NULL
);



CREATE SEQUENCE owners_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE owners_id_seq OWNED BY owners.id;



CREATE TABLE queue (
    id integer NOT NULL,
    name character varying(255) NOT NULL,
    version character varying(100) NOT NULL,
    attempt integer DEFAULT 0 NOT NULL,
    priority integer DEFAULT 0 NOT NULL,
    registry text,
    last_attempt timestamp with time zone
);



CREATE SEQUENCE queue_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE queue_id_seq OWNED BY queue.id;



CREATE TABLE releases (
    id integer NOT NULL,
    crate_id integer NOT NULL,
    version character varying(100) NOT NULL,
    release_time timestamp with time zone NOT NULL,
    dependencies json,
    target_name character varying(255) NOT NULL,
    yanked boolean DEFAULT false NOT NULL,
    is_library boolean DEFAULT true NOT NULL,
    build_status boolean DEFAULT false NOT NULL,
    rustdoc_status boolean DEFAULT false NOT NULL,
    test_status boolean DEFAULT false,
    license character varying(100),
    repository_url character varying(255),
    homepage_url character varying(255),
    documentation_url character varying(255),
    description character varying(1024),
    description_long character varying(51200),
    readme character varying(51200),
    keywords json,
    have_examples boolean DEFAULT false NOT NULL,
    downloads integer DEFAULT 0 NOT NULL,
    files json,
    doc_targets json DEFAULT '[]'::json NOT NULL,
    doc_rustc_version character varying(100) NOT NULL,
    default_target character varying(100) NOT NULL,
    features feature[],
    repository_id integer,
    archive_storage boolean DEFAULT false NOT NULL
);



CREATE SEQUENCE releases_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE releases_id_seq OWNED BY releases.id;



CREATE TABLE repositories (
    id integer NOT NULL,
    host character varying NOT NULL,
    host_id character varying NOT NULL,
    name character varying NOT NULL,
    description character varying,
    last_commit timestamp with time zone,
    stars integer NOT NULL,
    forks integer NOT NULL,
    issues integer NOT NULL,
    updated_at timestamp with time zone NOT NULL
);



CREATE SEQUENCE repositories_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



ALTER SEQUENCE repositories_id_seq OWNED BY repositories.id;



CREATE TABLE sandbox_overrides (
    crate_name character varying NOT NULL,
    max_memory_bytes bigint,
    timeout_seconds integer,
    max_targets integer
);



ALTER TABLE ONLY builds ALTER COLUMN id SET DEFAULT nextval('builds_id_seq'::regclass);



ALTER TABLE ONLY cdn_invalidation_queue ALTER COLUMN id SET DEFAULT nextval('cdn_invalidation_queue_id_seq'::regclass);



ALTER TABLE ONLY crates ALTER COLUMN id SET DEFAULT nextval('crates_id_seq'::regclass);



ALTER TABLE ONLY keywords ALTER COLUMN id SET DEFAULT nextval('keywords_id_seq'::regclass);



ALTER TABLE ONLY owners ALTER COLUMN id SET DEFAULT nextval('owners_id_seq'::regclass);



ALTER TABLE ONLY queue ALTER COLUMN id SET DEFAULT nextval('queue_id_seq'::regclass);



ALTER TABLE ONLY releases ALTER COLUMN id SET DEFAULT nextval('releases_id_seq'::regclass);



ALTER TABLE ONLY repositories ALTER COLUMN id SET DEFAULT nextval('repositories_id_seq'::regclass);



ALTER TABLE ONLY blacklisted_crates
    ADD CONSTRAINT blacklisted_crates_pkey PRIMARY KEY (crate_name);



ALTER TABLE ONLY compression_rels
    ADD CONSTRAINT compression_rels_release_algorithm_key UNIQUE (release, algorithm);



ALTER TABLE ONLY config
    ADD CONSTRAINT config_pkey PRIMARY KEY (name);



ALTER TABLE ONLY crate_priorities
    ADD CONSTRAINT crate_priorities_pattern_key UNIQUE (pattern);



ALTER TABLE ONLY crates
    ADD CONSTRAINT crates_name_key UNIQUE (name);



ALTER TABLE ONLY crates
    ADD CONSTRAINT crates_pkey PRIMARY KEY (id);



ALTER TABLE ONLY doc_coverage
    ADD CONSTRAINT doc_coverage_release_id_key UNIQUE (release_id);



ALTER TABLE ONLY files
    ADD CONSTRAINT files_pkey PRIMARY KEY (path);



ALTER TABLE ONLY keyword_rels
    ADD CONSTRAINT keyword_rels_rid_kid_key UNIQUE (rid, kid);



ALTER TABLE ONLY keywords
    ADD CONSTRAINT keywords_pkey PRIMARY KEY (id);



ALTER TABLE ONLY keywords
    ADD CONSTRAINT keywords_slug_key UNIQUE (slug);



ALTER TABLE ONLY owner_rels
    ADD CONSTRAINT owner_rels_cid_oid_key UNIQUE (cid, oid);



ALTER TABLE ONLY owners
    ADD CONSTRAINT owners_login_key UNIQUE (login);



ALTER TABLE ONLY owners
    ADD CONSTRAINT owners_pkey PRIMARY KEY (id);



ALTER TABLE ONLY queue
    ADD CONSTRAINT queue_name_version_key UNIQUE (name, version);



ALTER TABLE ONLY releases
    ADD CONSTRAINT releases_crate_id_version_key UNIQUE (crate_id, version);



ALTER TABLE ONLY releases
    ADD CONSTRAINT releases_pkey PRIMARY KEY (id);



ALTER TABLE ONLY repositories
    ADD CONSTRAINT repositories_host_host_id_key UNIQUE (host, host_id);



ALTER TABLE ONLY repositories
    ADD CONSTRAINT repositories_pkey PRIMARY KEY (id);



ALTER TABLE ONLY sandbox_overrides
    ADD CONSTRAINT sandbox_overrides_pkey PRIMARY KEY (crate_name);



CREATE INDEX builds_build_time_idx ON builds USING btree (build_time DESC);



CREATE INDEX builds_release_id_idx ON builds USING btree (rid);



CREATE INDEX cdn_invalidation_queue_cdn_reference_idx ON cdn_invalidation_queue USING btree (cdn_reference);



CREATE INDEX cdn_invalidation_queue_crate_idx ON cdn_invalidation_queue USING btree (crate);



CREATE INDEX cdn_invalidation_queue_created_in_cdn_idx ON cdn_invalidation_queue USING btree (created_in_cdn);



CREATE INDEX crates_latest_version_idx ON crates USING btree (latest_version_id);



CREATE UNIQUE INDEX crates_normalized_name_idx ON crates USING btree (normalize_crate_name(name));



CREATE INDEX releases_release_time_idx ON releases USING btree (release_time DESC);



CREATE INDEX releases_repo_idx ON releases USING btree (repository_id);



CREATE INDEX repos_stars_idx ON repositories USING btree (stars DESC);



ALTER TABLE ONLY builds
    ADD CONSTRAINT builds_rid_fkey FOREIGN KEY (rid) REFERENCES releases(id);



ALTER TABLE ONLY compression_rels
    ADD CONSTRAINT compression_rels_release_fkey FOREIGN KEY (release) REFERENCES releases(id);



ALTER TABLE ONLY doc_coverage
    ADD CONSTRAINT doc_coverage_release_id_fkey FOREIGN KEY (release_id) REFERENCES releases(id);



ALTER TABLE ONLY keyword_rels
    ADD CONSTRAINT keyword_rels_kid_fkey FOREIGN KEY (kid) REFERENCES keywords(id);



ALTER TABLE ONLY keyword_rels
    ADD CONSTRAINT keyword_rels_rid_fkey FOREIGN KEY (rid) REFERENCES releases(id);



ALTER TABLE ONLY owner_rels
    ADD CONSTRAINT owner_rels_cid_fkey FOREIGN KEY (cid) REFERENCES crates(id);



ALTER TABLE ONLY owner_rels
    ADD CONSTRAINT owner_rels_oid_fkey FOREIGN KEY (oid) REFERENCES owners(id);



ALTER TABLE ONLY releases
    ADD CONSTRAINT releases_crate_id_fkey FOREIGN KEY (crate_id) REFERENCES crates(id);



ALTER TABLE ONLY releases
    ADD CONSTRAINT releases_repository_id_fkey FOREIGN KEY (repository_id) REFERENCES repositories(id) ON DELETE SET NULL;


