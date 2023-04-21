---
sidebar_position: 1
---

# Fault Tolerance
As a cornerstone of mission-critical financial applications, Auticuro must ensure the highest 
levels of reliability and fault tolerance. Ensuring the system remains operational and responsive
even in the face of unexpected events or failures is crucial to maintaining user trust and 
safeguarding financial data. In the page, we will discuss the key strategies and design principles
that Auticuro employs to achieve exceptional fault tolerance, highlighting its resilience and
robustness in handling various challenges.

![image info](@site/static/img/design/firm-wallet-deployment.svg)

All components in the above Auticuro shard are fault tolerant. Let's analyze how to achieve fault tolerance
from the write side to the read side.

**Command Side**
* Network/Pod Failure

  A typical deployment of one Auticuro shard consists of 5 replicas(1 leader + 4 followers), which is
  available as long as the majority (n >= 3 in this case) are alive. If a follower undergoes a transient
  network failure or pod failure, the service would not be affected. Meanwhile, the failed follower
  would catch up when it recovers. If the leader fails, a new leader will be elected among the remaining
  4 followers, and the RTO (Recovery Time Objective) is less than 4 seconds.

* Disk Failure

  Auticuro periodically snapshots its state and uploads generated snapshots to cloud object storage,
  accelerating the recovery when a replica undergoes disk failure.

* Error Detection

  A recon engine incrementally pulls the raft log and event log from all 5 replicas in the background
  and compares them to detect potential divergence of the state machine. If divergence is detected,
  fault handling like rollback/roll forward is performed.

**Event Store**

Databases like PostgreSQL store recently pulled events and backups them to cloud object storage via CDC,
where CDC, a.k.a. Change Data Capture, refers to the process of identifying and capturing changes made
to data in a database and then delivering those changes in real-time to a downstream process or system.

If RDBMS undergoes temporary failure, services on the query side fall back to query events directly
from Auticuro. If RDBMS undergoes permanent failure, the event store recovers historical events from
cloud object storage and recent events from Auticuro.

**Query Side**

Services on the query side are stateless deployment in k8s, recovering from pod failure by rebuilding
the states from the event log.