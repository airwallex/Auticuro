## Firm Wallet Gateway Service

### Introduction
The firm wallet gateway service acts as a transparent proxy of the firm wallet service for ease of client integration.
The firm wallet gateway service is responsible for lead probing, and it will retry infinitely when the downstream firm 
wallet service is not available(during leader change) until success. 

### Configuration

See the configuration file at `.env`, including the service address of `DOWNSTREAM_SERVICE` and port for `Posting Service`
gRPC server.

### Run Service

1. Start the Firm Wallet Service
2. Start Firm Wallet Gateway Service

```
sh run_gateway.sh
```
