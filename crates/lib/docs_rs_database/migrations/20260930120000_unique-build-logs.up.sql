CREATE UNIQUE INDEX builds_logs_build_id_log_filename_idx
    ON builds_logs (build_id, log_filename);
