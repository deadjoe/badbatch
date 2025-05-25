----------------------------- MODULE SimpleRingBuffer -----------------------------
(***************************************************************************)
(* Simple Ring Buffer model for basic verification                        *)
(***************************************************************************)

LOCAL INSTANCE Naturals
LOCAL INSTANCE Integers
LOCAL INSTANCE FiniteSets

CONSTANTS
  Size,
  Readers,
  Writers,
  NULL

VARIABLE ringbuffer

LastIndex == Size - 1

(* Initial state of RingBuffer. *)
Init ==
  ringbuffer = [
    slots   |-> [i \in 0 .. LastIndex |-> NULL ],
    readers |-> [i \in 0 .. LastIndex |-> {}   ],
    writers |-> [i \in 0 .. LastIndex |-> {}   ]
  ]

(* The index into the Ring Buffer for a sequence number.*)
IndexOf(sequence) ==
  sequence % Size

(***************************************************************************)
(* Write operations.                                                       *)
(***************************************************************************)

Write(index, writer, value) ==
  ringbuffer' = [
    ringbuffer EXCEPT
      !.writers[index] = @ \union { writer },
      !.slots[index]   = value
  ]

EndWrite(index, writer) ==
  ringbuffer' = [ ringbuffer EXCEPT !.writers[index] = @ \ { writer } ]

(***************************************************************************)
(*  Read operations.                                                       *)
(***************************************************************************)

BeginRead(index, reader) ==
  ringbuffer' = [ ringbuffer EXCEPT !.readers[index] = @ \union { reader } ]

Read(index) ==
  ringbuffer.slots[index]

EndRead(index, reader) ==
  ringbuffer' = [ ringbuffer EXCEPT !.readers[index] = @ \ { reader } ]

(***************************************************************************)
(* Invariants.                                                             *)
(***************************************************************************)

TypeOk ==
  ringbuffer \in [
    slots:   [0 .. LastIndex -> Int \union { NULL }],
    readers: [0 .. LastIndex -> SUBSET(Readers)],
    writers: [0 .. LastIndex -> SUBSET(Writers)]
  ]

NoDataRaces ==
  \A i \in 0 .. LastIndex :
    /\ ringbuffer.readers[i] = {} \/ ringbuffer.writers[i] = {}
    /\ Cardinality(ringbuffer.writers[i]) <= 1

=============================================================================
