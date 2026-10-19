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
ALTER TABLE ONLY keywords ALTER COLUMN id SET DEFAULT nextval('keywords_id_seq'::regclass);
ALTER TABLE ONLY keywords
    ADD CONSTRAINT keywords_pkey PRIMARY KEY (id);
ALTER TABLE ONLY keywords
    ADD CONSTRAINT keywords_slug_key UNIQUE (slug);
ALTER TABLE ONLY keyword_rels
    ADD CONSTRAINT keyword_rels_kid_fkey FOREIGN KEY (kid) REFERENCES keywords(id);
