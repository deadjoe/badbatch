-------------------------------- MODULE BadBatchMPMC --------------------------------
(***************************************************************************)
(*  Models BadBatch Disruptor Multi Producer, Multi Consumer (MPMC).       *)
(* The model verifies that no data races occur between the publishers      *)
(* and consumers and that all consumers eventually read all published      *)
(* values.                                                                 *)
(*                                                                         *)
(* This is based on the proven disruptor-rs MPMC.tla model but adapted    *)
(* for BadBatch specific requirements.                                     *)
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

(* Get highest contiguous published sequence *)
HighestPublishedSequence ==
  LET published_seqs == { i \in 0..Len(availability_buffer)-1 :
                          availability_buffer[i + 1] = TRUE } IN
  IF published_seqs = {}
  THEN -1
  ELSE CHOOSE max_seq \in published_seqs :
         /\ \A i \in 0..max_seq : i \in published_seqs
         /\ \A other \in published_seqs : other <= max_seq

(***************************************************************************)
(* Producer actions modeling MultiProducerSequencer                       *)
(***************************************************************************)

(* Producer atomically claims next sequence number (models CAS operation) *)
ProducerClaimSequence(producer) ==
  /\ pc[producer] = Idle
  /\ LET seq == next_sequence IN
     /\ SequenceAvailable(seq)
     /\ next_sequence' = seq + 1
     /\ claimed_sequences' = [claimed_sequences EXCEPT ![producer] = seq]
     /\ pc' = [pc EXCEPT ![producer] = Claiming]
     /\ UNCHANGED << ringbuffer, availability_buffer, consumer_cursors,
                     consumed_events, wait_strategy >>

(* Producer writes event data to claimed slot *)
ProducerWriteEvent(producer) ==
  /\ pc[producer] = Claiming
  /\ LET
       seq == claimed_sequences[producer]
       index == Buffer!IndexOf(seq)
     IN
     /\ seq >= 0  (* Must have claimed a valid sequence *)
     /\ Buffer!BeginWrite(index, producer, seq)
     /\ pc' = [pc EXCEPT ![producer] = Writing]
     /\ UNCHANGED << next_sequence, claimed_sequences, availability_buffer,
                     consumer_cursors, consumed_events, wait_strategy >>

(* Producer publishes event by updating availability buffer *)
ProducerPublishEvent(producer) ==
  /\ pc[producer] = Writing
  /\ LET
       seq == claimed_sequences[producer]
       index == Buffer!IndexOf(seq)
     IN
     /\ seq >= 0
     /\ Buffer!EndWrite(index, producer)
     /\ availability_buffer' = [availability_buffer EXCEPT ![seq + 1] = TRUE]
     /\ claimed_sequences' = [claimed_sequences EXCEPT ![producer] = -1]
     /\ pc' = [pc EXCEPT ![producer] = Idle]
     /\ UNCHANGED << next_sequence, consumer_cursors, consumed_events,
                     wait_strategy >>

(***************************************************************************)
(* Consumer actions modeling EventProcessor behavior                       *)
(***************************************************************************)

(* Consumer waits for next available sequence *)
ConsumerWaitForSequence(consumer) ==
  /\ pc[consumer] = Idle
  /\ LET next_seq == consumer_cursors[consumer] + 1 IN
     /\ SequencePublished(next_seq)
     /\ next_seq <= HighestPublishedSequence
     /\ pc' = [pc EXCEPT ![consumer] = Reading]
     /\ UNCHANGED << ringbuffer, next_sequence, claimed_sequences,
                     availability_buffer, consumer_cursors, consumed_events,
                     wait_strategy >>

(* Consumer reads event from ring buffer *)
ConsumerReadEvent(consumer) ==
  /\ pc[consumer] = Reading
  /\ LET
       next_seq == consumer_cursors[consumer] + 1
       index == Buffer!IndexOf(next_seq)
     IN
     /\ Buffer!BeginRead(index, consumer)
     /\ pc' = [pc EXCEPT ![consumer] = Processing]
     /\ UNCHANGED << next_sequence, claimed_sequences, availability_buffer,
                     consumer_cursors, consumed_events, wait_strategy >>

(* Consumer processes event and advances cursor *)
ConsumerProcessEvent(consumer) ==
  /\ pc[consumer] = Processing
  /\ LET
       next_seq == consumer_cursors[consumer] + 1
       index == Buffer!IndexOf(next_seq)
       event_data == Buffer!ReadValue(index)
     IN
     /\ Buffer!EndRead(index, consumer)
     /\ consumer_cursors' = [consumer_cursors EXCEPT ![consumer] = next_seq]
     /\ consumed_events' = [consumed_events EXCEPT ![consumer] =
                            Append(@, event_data)]
     /\ pc' = [pc EXCEPT ![consumer] = Idle]
     /\ UNCHANGED << next_sequence, claimed_sequences, availability_buffer,
                     wait_strategy >>

(***************************************************************************)
(* System specification                                                    *)
(***************************************************************************)

Init ==
  /\ Buffer!Init
  /\ next_sequence = 0
  /\ claimed_sequences = [p \in Producers |-> -1]
  /\ availability_buffer = [i \in 1..MaxPublished |-> FALSE]
  /\ consumer_cursors = [c \in Consumers |-> -1]
  /\ consumed_events = [c \in Consumers |-> << >>]
  /\ wait_strategy = "BusySpin"
  /\ pc = [t \in Producers \union Consumers |-> Idle]

Next ==
  \/ \E p \in Producers : ProducerClaimSequence(p)
  \/ \E p \in Producers : ProducerWriteEvent(p)
  \/ \E p \in Producers : ProducerPublishEvent(p)
  \/ \E c \in Consumers : ConsumerWaitForSequence(c)
  \/ \E c \in Consumers : ConsumerReadEvent(c)
  \/ \E c \in Consumers : ConsumerProcessEvent(c)

(***************************************************************************)
(* Fairness conditions                                                     *)
(***************************************************************************)

Fairness ==
  /\ \A p \in Producers : WF_vars(ProducerClaimSequence(p))
  /\ \A p \in Producers : WF_vars(ProducerWriteEvent(p))
  /\ \A p \in Producers : WF_vars(ProducerPublishEvent(p))
  /\ \A c \in Consumers : WF_vars(ConsumerWaitForSequence(c))
  /\ \A c \in Consumers : WF_vars(ConsumerReadEvent(c))
  /\ \A c \in Consumers : WF_vars(ConsumerProcessEvent(c))

Spec ==
  Init /\ [][Next]_vars /\ Fairness

(***************************************************************************)
(* Type safety invariants                                                  *)
(***************************************************************************)

TypeOk ==
  /\ Buffer!TypeOk
  /\ next_sequence \in Nat
  /\ claimed_sequences \in [Producers -> Int]
  /\ availability_buffer \in [1..MaxPublished -> {TRUE, FALSE}]
  /\ consumer_cursors \in [Consumers -> Int]
  /\ consumed_events \in [Consumers -> Seq(Int \union {NULL})]
  /\ wait_strategy \in WaitStrategyStates
  /\ pc \in [Producers \union Consumers ->
             {Idle, Claiming, Writing, Publishing, Reading, Processing}]

(***************************************************************************)
(* Safety properties                                                       *)
(***************************************************************************)

(* No data races in ring buffer access *)
NoDataRaces == Buffer!NoDataRaces

(* Each sequence is claimed by at most one producer *)
UniqueSequenceClaims ==
  \A p1, p2 \in Producers :
    /\ p1 # p2
    /\ claimed_sequences[p1] >= 0
    /\ claimed_sequences[p2] >= 0
    => claimed_sequences[p1] # claimed_sequences[p2]

(* Claimed sequences are within valid range *)
ValidClaimedSequences ==
  \A p \in Producers :
    claimed_sequences[p] >= -1 /\ claimed_sequences[p] < MaxPublished

(* Consumer cursors never exceed highest published sequence *)
ConsumerConsistency ==
  \A c \in Consumers :
    consumer_cursors[c] <= HighestPublishedSequence

(* Availability buffer consistency *)
AvailabilityBufferConsistency ==
  \A i \in 1..MaxPublished :
    availability_buffer[i] = TRUE =>
      \E p \in Producers :
        /\ pc[p] \in {Idle}  (* Producer has completed publishing *)
        \/ (pc[p] = Publishing /\ claimed_sequences[p] = i - 1)

(***************************************************************************)
(* Liveness properties                                                     *)
(***************************************************************************)

(* All consumers eventually consume all published events *)
EventualConsumption ==
  <>[] (\A c \in Consumers :
         consumer_cursors[c] = HighestPublishedSequence)

(* Producers make progress when sequences are available *)
ProducerProgress ==
  \A p \in Producers :
    (pc[p] = Idle /\ next_sequence < MaxPublished) ~>
    (claimed_sequences[p] >= 0)

(* Consumers make progress when events are available *)
ConsumerProgress ==
  \A c \in Consumers :
    []((consumer_cursors[c] < HighestPublishedSequence) =>
       <>(consumer_cursors[c] > consumer_cursors[c]))

(***************************************************************************)
(* Composite properties                                                    *)
(***************************************************************************)

SafetyProperties ==
  /\ TypeOk
  /\ NoDataRaces
  /\ UniqueSequenceClaims
  /\ ValidClaimedSequences
  /\ ConsumerConsistency
  /\ AvailabilityBufferConsistency

LivenessProperties ==
  /\ EventualConsumption

=============================================================================
