CREATE OR REPLACE VIEW release_build_status AS (
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
  ) as summary
);
