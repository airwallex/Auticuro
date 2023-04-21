---
sidebar_position: 1
---

# Test Scenarios


## Test Scenarios
### Transfer
* **The Scenario**:

Given accounts A and B, whose initial balance is 0, transfer money between A and B for N times, the
total amount of the N transfer is 0(see E1 below), thus accounts A and B’s balance should be 0 after 
processing the Nth transfer request. (N = 10000 for test implementation)


* **Example** (N = 3 for illustration):


<table>
  <tr>
   <td>A -> B
   </td>
   <td>10
   </td>
  </tr>
  <tr>
   <td>B -> A
   </td>
   <td>30
   </td>
  </tr>
  <tr>
   <td>A -> B
   </td>
   <td>20
   </td>
  </tr>
</table>

Balance of A and B should stay unchanged:


    A:  (-10) + 30 + (-20) = 0
    
    B:  10 + (-30) + 20 = 0

### BatchBalanceOperation

* **The Scenario**:

  Given accounts A, B, and C, make **N(**N=10000 for testing**)**  BatchBalanceOperation(BBO) 
requests involving accounts A, B, and C. After all the requests have been handled, the balance 
of A, B, and C should stay unchanged if the below requirements are satisfied:

* **Example** (N = 4):

<table>
  <tr>
   <td>
   </td>
   <td>
A
   </td>
   <td>B
   </td>
   <td>C
   </td>
  </tr>
  <tr>
   <td>BBO1
   </td>
   <td>1
   </td>
   <td>7
   </td>
   <td>-3
   </td>
  </tr>
  <tr>
   <td>BBO2
   </td>
   <td>9
   </td>
   <td>8
   </td>
   <td>8
   </td>
  </tr>
  <tr>
   <td>BBO3
   </td>
   <td>-7
   </td>
   <td>-10
   </td>
   <td>-1
   </td>
  </tr>
  <tr>
   <td>BBO4
   </td>
   <td>-3
   </td>
   <td>-5
   </td>
   <td>-4
   </td>
  </tr>
</table>

Balance of A, B, and C should stay unchanged:
```
    A: 1 + 9 + (-7) + (-3) == 0
    B: 7 + 8 + (-10) + (-5) == 0
    C: (-3) + (8) + (-1) + (-4) == 0
```

### Reserve/Release

* **The Scenario:**

  Given an account A, make N(N=100 for testing) reservations and N full releases. After all the 
requests have been handled, the balance of account A should remain unchanged, and there should be
no ongoing reservations. A release request must be put after its corresponding reserve request, 
but it is not required to be adjacent.

* **Example(N=3):**

  In this case, after step 4, there should be two reservation entries (id_1 -> 100, id_3 -> 300).
  When all the requests have been handled, there should be no ongoing reservations, and the balance 
  of account A should stay unchanged.


<table>
  <tr>
   <td>
   </td>
   <td>
Reservation_id
   </td>
   <td>Amount
   </td>
  </tr>
  <tr>
   <td>
    Reserve
   </td>
   <td>id_1
   </td>
   <td>100
   </td>
  </tr>
  <tr>
   <td>
    Reserve
   </td>
   <td>id_2
   </td>
   <td>200
   </td>
  </tr>
  <tr>
   <td>
    Release
   </td>
   <td>id_2
   </td>
   <td>200
   </td>
  </tr>
  <tr>
   <td>
    Reserve
   </td>
   <td>id_3
   </td>
   <td>300
   </td>
  </tr>
  <tr>
   <td>
    Release
   </td>
   <td>id_1
   </td>
   <td>100
   </td>
  </tr>
  <tr>
   <td>
    Release
   </td>
   <td>id_3
   </td>
   <td>300
   </td>
  </tr>
</table>


### TCCTry/Confirm/Cancel

* **The Scenario(Try+Confirm)**

  Given accounts A, B, and C, make N TccTry and N TccConfirm requests against A, B, and C. 
A TccConfirm request must be after its corresponding TccTry request, but it is not required to be adjacent.

Prerequisites: for accounts A, B, and C, the total amount in Try requests should be equal to 0 as below:

* **Example(N=3)**

  There might be ongoing transactions before the N (Try+ Confirm) pairs be handled, while
  after all requests have been handled, the balance of A, B, and C should stage **unchanged**.

  Balance change of accounts A, B, and C should be 0:


    A: -2 + 8 + (-6) = 0

    B: -4 + 7 + (-3) = 0

    C: 12 + (-2) + (-10) = 0


<table>
  <tr>
   <td>
   </td>
   <td>A
   </td>
   <td>B
   </td>
   <td>C
   </td>
  </tr>
  <tr>
   <td>Try(txn_1)
   </td>
   <td>-2
   </td>
   <td>-4
   </td>
   <td>12
   </td>
  </tr>
  <tr>
   <td>Try(txn_2)
   </td>
   <td>8
   </td>
   <td>7
   </td>
   <td>-2
   </td>
  </tr>
  <tr>
   <td>Confirm(txn_2)
   </td>
   <td>
   </td>
   <td>
   </td>
   <td>
   </td>
  </tr>
  <tr>
   <td>Try(txn_3)
   </td>
   <td>-6
   </td>
   <td>-3
   </td>
   <td>-10
   </td>
  </tr>
  <tr>
   <td>Confirm(txn_1)
   </td>
   <td>
   </td>
   <td>
   </td>
   <td>
   </td>
  </tr>
  <tr>
   <td>Confirm(txn_3)
   </td>
   <td>
   </td>
   <td>
   </td>
   <td>
   </td>
  </tr>
</table>

