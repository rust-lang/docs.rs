-- content copy/pasted from `.._initial.up.sql`

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
ALTER TABLE ONLY cdn_invalidation_queue ALTER COLUMN id SET DEFAULT nextval('cdn_invalidation_queue_id_seq'::regclass);

CREATE INDEX cdn_invalidation_queue_cdn_reference_idx ON cdn_invalidation_queue USING btree (cdn_reference);
CREATE INDEX cdn_invalidation_queue_crate_idx ON cdn_invalidation_queue USING btree (crate);
CREATE INDEX cdn_invalidation_queue_created_in_cdn_idx ON cdn_invalidation_queue USING btree (created_in_cdn);
