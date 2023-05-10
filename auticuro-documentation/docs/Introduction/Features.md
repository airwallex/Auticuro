---
sidebar_position: 1
---

# Auticuro Features
Auticuro is designed to provide a seamless and efficient asset management experience for users. With an extensive list of functional and non-functional features, Auticuro aims to cater to a wide range of applications and use cases. In this section, we will walk you through the key features that make Auticuro a standout choice for businesses and individuals looking to manage their financial assets with ease and precision.
Below is the features overview of Auticuro's features that contribute to its exceptional 
performance and versatility:

## Functional Features
- Balance operations
    - ✅ Bilateral money transfer
    - ✅ Batch balance operation (Increase/Decrease balance on multiple accounts atomically)
    - ✅ Reserve/IncreasingReserve/PartialRelease/FullRelease
    - ✅ TCC/SAGA balance operation APIs
- Account management operations
    - ✅ Create/Delete/Lock/Unlock an account
    - ✅ Change the upper/lower balance limit of an account
- ✅ Support multi asset class (cash, crypto, coupon…)
- ✅ Support Balance history query
- ☐ Support account hierarchy(TBD)

## Non-functional Features
- **High throughput**: Separated reads and writes, no lock on the critical write path
- **High availability**: Built upon a Raft-based consensus message queue to achieve HA
- **Low latency**: The wallet service pushes down computation to the storage layer to achieve low latency
- **Horizontal scalability**: The horizontal scalability can be achieved with the help of an in-house transaction manager(**Marker**)
