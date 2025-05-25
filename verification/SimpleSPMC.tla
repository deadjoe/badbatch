----------------------------- MODULE SimpleSPMC -----------------------------
(***************************************************************************)
(* Simple Single Producer Multi Consumer model for basic verification     *)
(***************************************************************************)

EXTENDS Naturals, Integers, FiniteSets, Sequences

CONSTANTS
  Producer,
  Consumers,
  MaxPublished,
  Size,
  NULL

VARIABLES
  ringbuffer,
  published,
  read,
  consumed,
  pc

vars == <<
  ringbuffer,
  published,
  read,
  consumed,
  pc
>>

Access  == "Access"
Advance == "Advance"

Transition(t, from, to) ==
  /\ pc[t] = from
  /\ pc'   = [ pc EXCEPT ![t] = to ]

Buffer  == INSTANCE SimpleRingBuffer WITH
  Readers <- Consumers,
  Writers <- {Producer}

Range(f) ==
  { f[x] : x \in DOMAIN(f) }

MinReadSequence ==
  IF Consumers = {}
  THEN published
  ELSE CHOOSE min \in Range(read) : \A r \in Consumers : min <= read[r]

(***************************************************************************)
(* Producer Actions:                                                       *)
(***************************************************************************)

BeginWrite ==
  LET
    next     == published + 1
    index    == Buffer!IndexOf(next)
    min_read == MinReadSequence
  IN
    /\ min_read >= next - Size
    /\ next < MaxPublished
    /\ Transition(Producer, Access, Advance)
    /\ Buffer!Write(index, Producer, next)
    /\ UNCHANGED << consumed, published, read >>

EndWrite ==
  LET
    next  == published + 1
    index == Buffer!IndexOf(next)
  IN
    /\ Transition(Producer, Advance, Access)
    /\ Buffer!EndWrite(index, Producer)
    /\ published' = next
    /\ UNCHANGED << consumed, read >>

(***************************************************************************)
(* Consumer Actions:                                                       *)
(***************************************************************************)

BeginRead(reader) ==
  LET
    next  == read[reader] + 1
    index == Buffer!IndexOf(next)
  IN
    /\ published >= next
    /\ Transition(reader, Access, Advance)
    /\ Buffer!BeginRead(index, reader)
    /\ consumed' = [ consumed EXCEPT ![reader] = Append(@, Buffer!Read(index)) ]
    /\ UNCHANGED << published, read >>

EndRead(reader) ==
  LET
    next  == read[reader] + 1
    index == Buffer!IndexOf(next)
  IN
    /\ Transition(reader, Advance, Access)
    /\ Buffer!EndRead(index, reader)
    /\ read' = [ read EXCEPT ![reader] = next ]
    /\ UNCHANGED << consumed, published >>

(***************************************************************************)
(* Spec:                                                                   *)
(***************************************************************************)

Init ==
  /\ Buffer!Init
  /\ published = -1
  /\ read      = [ r \in Consumers |-> -1 ]
  /\ consumed  = [ r \in Consumers |-> << >> ]
  /\ pc        = [ a \in {Producer} \union Consumers |-> Access ]

Next ==
  \/ BeginWrite
  \/ EndWrite
  \/ \E r \in Consumers : BeginRead(r)
  \/ \E r \in Consumers : EndRead(r)

Fairness ==
  /\ WF_vars(BeginWrite)
  /\ WF_vars(EndWrite)
  /\ \A r \in Consumers : WF_vars(BeginRead(r))
  /\ \A r \in Consumers : WF_vars(EndRead(r))

Spec ==
  Init /\ [][Next]_vars /\ Fairness

(***************************************************************************)
(* Invariants:                                                             *)
(***************************************************************************)

TypeOk ==
  /\ Buffer!TypeOk
  /\ published \in Int
  /\ read      \in [ Consumers -> Int ]
  /\ consumed  \in [ Consumers -> Seq(Nat) ]
  /\ pc        \in [ {Producer} \union Consumers -> { Access, Advance } ]

NoDataRaces == Buffer!NoDataRaces

EventualConsumption ==
  <>[] (\A r \in Consumers : Len(consumed[r]) >= MaxPublished)

=============================================================================
