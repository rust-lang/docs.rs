DROP VIEW release_build_status;
CREATE TABLE release_build_status (
    rid INTEGER NOT NULL PRIMARY KEY REFERENCES releases ON DELETE CASCADE,
    last_build_time timestamp with time zone,
    build_status build_status NOT NULL
);


INSERT INTO release_build_status(rid, last_build_time, build_status) 
SELECT 
summary.id,
summary.last_build_time,
CASE 
  WHEN summary.success_count > 0 THEN 'success'::build_status
  WHEN summary.failure_count > 0 THEN 'failure'::build_status
  ELSE 'in_progress'::build_status
END as build_status
  
FROM (
    SELECT
      r.id,
      MAX(b.build_time) as last_build_time,
      SUM(CASE WHEN b.build_status = 'success' THEN 1 ELSE 0 END) as success_count,
      SUM(CASE WHEN b.build_status = 'failure' THEN 1 ELSE 0 END) as failure_count
    FROM 
      releases as r
      LEFT OUTER JOIN builds AS b on b.rid = r.id 
    GROUP BY r.id
) as summary;

CREATE INDEX release_build_status_last_build_time_idx ON release_build_status USING btree (last_build_time DESC);
CREATE INDEX release_build_status_build_status_idx ON release_build_status USING btree (build_status);
