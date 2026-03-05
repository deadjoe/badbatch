--------------------------- MODULE BadBatchPipeline ---------------------------
(***************************************************************************)
(* Models BadBatch Disruptor with consumer dependency chains (pipeline).   *)
(*                                                                         *)
(* In a pipeline, Stage2 consumers may only process an event after ALL     *)
(* Stage1 consumers have finished processing it.  This models the          *)
(* `and_then` dependency API in badbatch's builder.                        *)
(*                                                                         *)
(* The model verifies that:                                                *)
(* 1. No data races occur between producer and any consumer stage          *)
(* 2. Stage2 never processes ahead of Stage1 (dependency safety)           *)
(* 3. All Stage2 consumers eventually consume all published events         *)
(***************************************************************************)

EXTENDS Naturals, Integers, FiniteSets, Sequences

CONSTANTS
  Writers,          (* Writer/publisher thread ids (single producer).  *)
  Stage1,           (* First-stage consumer thread ids.                *)
  Stage2,           (* Second-stage consumer thread ids (depend on Stage1). *)
  MaxPublished,     (* Max number of published events.                 *)
  Size,             (* Ringbuffer size.                                *)
  NULL

VARIABLES
  ringbuffer,
  published,        (* Publisher cursor.                               *)
  read1,            (* Read cursors for Stage1 consumers.              *)
  read2,            (* Read cursors for Stage2 consumers.              *)
  consumed1,        (* Sequence of all read events by Stage1.          *)
  consumed2,        (* Sequence of all read events by Stage2.          *)
  pc                (* Program Counter of each thread.                 *)

vars == <<
  ringbuffer,
  published,
  read1,
  read2,
  consumed1,
  consumed2,
  pc
>>

(***************************************************************************)
(* Each thread can be in one of two states:                                *)
(* 1. Accessing a slot in the Disruptor or                                 *)
(* 2. Advancing to the next slot.                                          *)
(***************************************************************************)
Access  == "Access"
Advance == "Advance"

Transition(t, from, to) ==
  /\ pc[t] = from
  /\ pc'   = [ pc EXCEPT ![t] = to ]

Buffer == INSTANCE BadBatchRingBuffer WITH
  Readers <- Stage1 \union Stage2,
  Writers <- Writers

Range(f) ==
  { f[x] : x \in DOMAIN(f) }

(***************************************************************************)
(* Minimum read cursors for each stage.                                    *)
(* MinStage1 is the dependency barrier for Stage2 consumers.               *)
(* MinAllReads is the backpressure barrier for the producer.               *)
(***************************************************************************)
MinStage1 ==
  CHOOSE min \in Range(read1) : \A s \in Stage1 : min <= read1[s]

MinStage2 ==
  CHOOSE min \in Range(read2) : \A s \in Stage2 : min <= read2[s]

MinAllReads ==
  IF MinStage1 <= MinStage2 THEN MinStage1 ELSE MinStage2

(***************************************************************************)
(* Publisher Actions (single producer, cursor-based):                      *)
(***************************************************************************)

BeginWrite(writer) ==
  LET
    next     == published + 1
    index    == Buffer!IndexOf(next)
    min_read == MinAllReads
  IN
    /\ min_read >= next - Size
    /\ next < MaxPublished
    /\ Transition(writer, Access, Advance)
    /\ Buffer!Write(index, writer, next)
    /\ UNCHANGED << consumed1, consumed2, published, read1, read2 >>

EndWrite(writer) ==
  LET
    next  == published + 1
    index == Buffer!IndexOf(next)
  IN
    /\ Transition(writer, Advance, Access)
    /\ Buffer!EndWrite(index, writer)
    /\ published' = next
    /\ UNCHANGED << consumed1, consumed2, read1, read2 >>

(***************************************************************************)
(* Stage1 Consumer Actions — depends only on the producer cursor.          *)
(***************************************************************************)

BeginReadStage1(reader) ==
  LET
    next  == read1[reader] + 1
    index == Buffer!IndexOf(next)
  IN
    /\ published >= next
    /\ Transition(reader, Access, Advance)
    /\ Buffer!BeginRead(index, reader)
    /\ consumed1' = [ consumed1 EXCEPT ![reader] = Append(@, Buffer!Read(index)) ]
    /\ UNCHANGED << published, read1, read2, consumed2 >>

EndReadStage1(reader) ==
  LET
    next  == read1[reader] + 1
    index == Buffer!IndexOf(next)
  IN
    /\ Transition(reader, Advance, Access)
    /\ Buffer!EndRead(index, reader)
    /\ read1' = [ read1 EXCEPT ![reader] = next ]
    /\ UNCHANGED << consumed1, consumed2, published, read2 >>

(***************************************************************************)
(* Stage2 Consumer Actions — depends on ALL Stage1 consumers.              *)
(* This models the `and_then` dependency chain in badbatch's builder API.  *)
(***************************************************************************)

BeginReadStage2(reader) ==
  LET
    next  == read2[reader] + 1
    index == Buffer!IndexOf(next)
  IN
    /\ published >= next          \* Sequence must be published
    /\ MinStage1 >= next          \* All Stage1 consumers must have processed it
    /\ Transition(reader, Access, Advance)
    /\ Buffer!BeginRead(index, reader)
    /\ consumed2' = [ consumed2 EXCEPT ![reader] = Append(@, Buffer!Read(index)) ]
    /\ UNCHANGED << published, read1, read2, consumed1 >>

EndReadStage2(reader) ==
  LET
    next  == read2[reader] + 1
    index == Buffer!IndexOf(next)
  IN
    /\ Transition(reader, Advance, Access)
    /\ Buffer!EndRead(index, reader)
    /\ read2' = [ read2 EXCEPT ![reader] = next ]
    /\ UNCHANGED << consumed1, consumed2, published, read1 >>

(***************************************************************************)
(* Spec:                                                                   *)
(***************************************************************************)

Init ==
  /\ Buffer!Init
  /\ published = -1
  /\ read1     = [ r \in Stage1 |-> -1     ]
  /\ read2     = [ r \in Stage2 |-> -1     ]
  /\ consumed1 = [ r \in Stage1 |-> << >>  ]
  /\ consumed2 = [ r \in Stage2 |-> << >>  ]
  /\ pc        = [ a \in Writers \union Stage1 \union Stage2 |-> Access ]

Next ==
  \/ \E w \in Writers : BeginWrite(w)
  \/ \E w \in Writers : EndWrite(w)
  \/ \E r \in Stage1  : BeginReadStage1(r)
  \/ \E r \in Stage1  : EndReadStage1(r)
  \/ \E r \in Stage2  : BeginReadStage2(r)
  \/ \E r \in Stage2  : EndReadStage2(r)

Fairness ==
  /\ \A w \in Writers : WF_vars(BeginWrite(w))
  /\ \A w \in Writers : WF_vars(EndWrite(w))
  /\ \A r \in Stage1  : WF_vars(BeginReadStage1(r))
  /\ \A r \in Stage1  : WF_vars(EndReadStage1(r))
  /\ \A r \in Stage2  : WF_vars(BeginReadStage2(r))
  /\ \A r \in Stage2  : WF_vars(EndReadStage2(r))

Spec ==
  Init /\ [][Next]_vars /\ Fairness

(***************************************************************************)
(* Invariants:                                                             *)
(***************************************************************************)

NoDataRaces == Buffer!NoDataRaces

TypeOk ==
  /\ Buffer!TypeOk
  /\ published \in Int
  /\ read1     \in [ Stage1 -> Int       ]
  /\ read2     \in [ Stage2 -> Int       ]
  /\ consumed1 \in [ Stage1 -> Seq(Nat)  ]
  /\ consumed2 \in [ Stage2 -> Seq(Nat)  ]
  /\ pc        \in [ Writers \union Stage1 \union Stage2 -> { Access, Advance } ]

(* Safety: Stage2 consumers never process ahead of any Stage1 consumer. *)
(* This is the core pipeline dependency invariant.                       *)
DependencyOrder ==
  \A s2 \in Stage2 : \A s1 \in Stage1 :
    read2[s2] <= read1[s1]

(***************************************************************************)
(* Properties:                                                             *)
(***************************************************************************)

(* All Stage2 consumers eventually consume all published events *)
Liveness ==
  <>[] (\A r \in Stage2 : consumed2[r] = [i \in 1..MaxPublished |-> i - 1])

=============================================================================
