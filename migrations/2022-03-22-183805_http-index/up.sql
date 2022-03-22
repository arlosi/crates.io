ALTER TABLE dependencies
    ADD COLUMN explicit_name VARCHAR NULL;
ALTER TABLE versions
    ADD COLUMN checksum CHAR(64) NULL;