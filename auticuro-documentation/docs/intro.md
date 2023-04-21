---
sidebar_position: 1
---

# Auticuro Overview

**Auticuro** is a high performance, strong consistent, distributed wallet service fitting for mission-critical financial
applications. It provides account management and balance operation functionalities, which could be used to build composite
money movement functionalities required by customers.

Besides providing **exactly-once** money movement guarantees, **Auticuro** also supports TCC/SAGA balance operation APIs to
ensure atomicity for multiple balance operations.

**Auticuro** has predictable low latency(**P99 < 20ms** when TPS = **10K**, tested against a 5-node deployment on GCP)
and high-availability(RTO <= **4s** for fault recovery), which make it a suitable cornerstone for critical financial
applications.

## Auticuro Features Overview
**Functional:**
- **Payment processing**: Ensures that money movement or payment is processed without duplication or 
loss.
- **Account Creation and Management**: Allows users to create and manage accounts, providing a single 
  platform for managing all accounts and reducing the risk of errors and discrepancies.
- **Account Hierarchy View**: Allows users to create a tree-style structure of accounts, making it 
  easier to organize and track transactions across multiple accounts. Provides a single view of the total balance across all accounts and sub-accounts, allowing users to assess their financial position quickly.
- **Transaction History and Auditing**: The wallet service should have an accurate and immutable 
  history as the source of truth,  providing explainability and traceability for auditing or regulatory requirements.

**Non-Functional:**
- **Availability**: Provides redundancy and failover mechanisms to ensure system availability and 
data integrity in the event of technical issues.
- **Low-latency**: Provides real-time processing of transactions, enabling users to make payments and 
  transfers quickly and efficiently.
- **Scalability**: Implements a distributed architecture to ensure the system can handle increased 
  traffic and demand as the user base grows.
- **Evolvability**: Decouples business logic and fundamental APIs, making the system easily adapt to 
  changing business requirements or customer needs without requiring a complete overhaul of the entire system.