# legacy infrastructure

Here's a simplified graph of the different moving pieces.

```mermaid
flowchart LR
  user[User] --> fastly[Fastly CDN]
  fastly <--> ngwaf[Fastly NgWAF]

  subgraph ec2[EC2 server]
    nginx[nginx] --> web[webserver] --> |accesses| psql[postgres database]
    watcher[index watcher] --> |enqueues builds| psql
    builder[builder * 4] --> |reads queued builds| psql
  end

  fastly --> nginx
  web -->|accesses| s3[AWS S3]
  builder --> |uploads docs| s3

  index[crates.io git index] --> |gets pulled by| watcher
```
