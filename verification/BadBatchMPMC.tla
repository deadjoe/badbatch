-------------------------------- MODULE BadBatchMPMC --------------------------------
(***************************************************************************)
(*  Models BadBatch Disruptor Multi Producer, Multi Consumer (MPMC).       *)
(* The model verifies that no data races occur between the publishers      *)
(* and consumers and that all consumers eventually read all published      *)
(* values.                                                                 *)
(*                                                                         *)
(* This is based on the proven disruptor-rs MPMC.tla model.               *)
(***************************************************************************)

EXTENDS Naturals, Integers, FiniteSets, Sequences

CONSTANTS
  Writers,          (* Writer/publisher thread ids.    *)
  Readers,          (* Reader/consumer  thread ids.    *)
  MaxPublished,     (* Max number of published events. *)
  Size,             (* Ringbuffer size.                *)
  NULL

VARIABLES
  ringbuffer,
  next_sequence,    (* Shared counter for claiming a sequence for a Writer. *)
  claimed_sequence, (* Claimed sequence by each Writer.                     *)
  published,        (* Encodes whether each slot is published.              *)
  read,             (* Read Cursors. One per Reader.                        *)
  consumed,         (* Sequence of all read events by the Reader.           *)
  pc                (* Program Counter of each Writer/Reader.               *)

vars == <<
  ringbuffer,
  next_sequence,
  claimed_sequence,
  published,
  read,
  consumed,
  pc
>>

(***************************************************************************)
(* Each publisher/consumer can be in one of two states:                    *)
(* 1. Accessing a slot in the Disruptor or                                 *)
(* 2. Advancing to the next slot.                                          *)
(***************************************************************************)
Access  == "Access"
Advance == "Advance"

Transition(t, from, to) ==
  /\ pc[t] = from
  /\ pc'   = [ pc EXCEPT ![t] = to ]

Buffer  == INSTANCE BadBatchRingBuffer

Range(f) ==
  { f[x] : x \in DOMAIN(f) }

MinReadSequence ==
  CHOOSE min \in Range(read) : \A r \in Readers : min <= read[r]

(***************************************************************************)
(* Checks if a sequence number is published.                               *)
(* This models the availability buffer mechanism.                          *)
(***************************************************************************)
IsPublished(sequence) ==
  LET
    index == Buffer!IndexOf(sequence)
    turn  == sequence \div Size
  IN
    published[index] = turn

(***************************************************************************)
(* Publisher Actions:                                                      *)
(***************************************************************************)

ClaimSequence(writer) ==
  LET
    sequence  == next_sequence
    index     == Buffer!IndexOf(sequence)
    min_read  == MinReadSequence
  IN
    \* Are we clear of all consumers? (Potentially a full cycle behind).
    /\ min_read >= sequence - Size
    /\ sequence < MaxPublished
    /\ Transition(writer, Access, Advance)
    /\ next_sequence' = next_sequence + 1
    /\ claimed_sequence' = [ claimed_sequence EXCEPT ![writer] = sequence ]
    /\ UNCHANGED << ringbuffer, published, read, consumed >>

BeginWrite(writer) ==
  LET
    sequence == claimed_sequence[writer]
    index    == Buffer!IndexOf(sequence)
  IN
    /\ sequence >= 0  (* Must have claimed a valid sequence *)
    /\ Transition(writer, Advance, Access)
    /\ Buffer!Write(index, writer, sequence)
    /\ UNCHANGED << next_sequence, claimed_sequence, published, read, consumed >>

EndWrite(writer) ==
  LET
    sequence == claimed_sequence[writer]
    index    == Buffer!IndexOf(sequence)
    turn     == sequence \div Size
  IN
    /\ sequence >= 0  (* Must have claimed a valid sequence *)
    /\ Transition(writer, Access, Advance)
    /\ Buffer!EndWrite(index, writer)
    /\ published' = [ published EXCEPT ![index] = turn ]
    /\ UNCHANGED << next_sequence, claimed_sequence, read, consumed >>



(***************************************************************************)
(* Consumer Actions:                                                       *)
(***************************************************************************)

BeginRead(reader) ==
  LET
    next     == read[reader] + 1
    index    == Buffer!IndexOf(next)
  IN
    /\ IsPublished(next)
    /\ Transition(reader, Access, Advance)
    /\ Buffer!BeginRead(index, reader)
    \* Track what we read from the ringbuffer.
    /\ consumed' = [ consumed EXCEPT ![reader] = Append(@, Buffer!Read(index)) ]
    /\ UNCHANGED << next_sequence, claimed_sequence, published, read >>

EndRead(reader) ==
  LET
    next  == read[reader] + 1
    index == Buffer!IndexOf(next)
  IN
    /\ Transition(reader, Advance, Access)
    /\ Buffer!EndRead(index, reader)
    /\ read' = [ read EXCEPT ![reader] = next ]
    /\ UNCHANGED << next_sequence, claimed_sequence, published, consumed >>

(***************************************************************************)
(* Spec:                                                                   *)
(***************************************************************************)

Init ==
  /\ Buffer!Init
  /\ next_sequence = 0
  /\ claimed_sequence = [ w \in Writers |-> -1 ]
  /\ published = [ i \in 0 .. (Size - 1) |-> -1 ]
  /\ read      = [ r \in Readers                |-> -1     ]
  /\ consumed  = [ r \in Readers                |-> << >>  ]
  /\ pc        = [ a \in Writers \union Readers |-> Access ]

Next ==
  \/ \E w \in Writers : ClaimSequence(w)
  \/ \E w \in Writers : BeginWrite(w)
  \/ \E w \in Writers : EndWrite(w)
  \/ \E r \in Readers : BeginRead(r)
  \/ \E r \in Readers : EndRead(r)

Fairness ==
  /\ \A w \in Writers : WF_vars(ClaimSequence(w))
  /\ \A w \in Writers : WF_vars(BeginWrite(w))
  /\ \A w \in Writers : WF_vars(EndWrite(w))
  /\ \A r \in Readers : WF_vars(BeginRead(r))
  /\ \A r \in Readers : WF_vars(EndRead(r))

Spec ==
  Init /\ [][Next]_vars /\ Fairness

(***************************************************************************)
(* Invariants:                                                             *)
(***************************************************************************)

TypeOk ==
  /\ Buffer!TypeOk
  /\ next_sequence \in Nat
  /\ claimed_sequence \in [ Writers -> Int ]
  /\ published \in [ 0 .. (Size - 1) -> Int ]
  /\ read      \in [ Readers                -> Int                 ]
  /\ consumed  \in [ Readers                -> Seq(Nat)            ]
  /\ pc        \in [ Writers \union Readers -> { Access, Advance } ]

NoDataRaces == Buffer!NoDataRaces

UniqueSequenceClaims ==
  \A w1, w2 \in Writers :
    /\ w1 # w2
    /\ claimed_sequence[w1] >= 0
    /\ claimed_sequence[w2] >= 0
    => claimed_sequence[w1] # claimed_sequence[w2]

ValidClaimedSequences ==
  \A w \in Writers :
    claimed_sequence[w] >= 0 => claimed_sequence[w] < next_sequence

ConsumerConsistency ==
  \A r \in Readers : read[r] >= -1

(***************************************************************************)
(* Properties:                                                             *)
(***************************************************************************)

EventualConsumption ==
  <>[] (\A r \in Readers : Len(consumed[r]) >= MaxPublished)

=============================================================================
