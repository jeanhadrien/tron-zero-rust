# Network & Simulation Architecture (Netcode)

## 1. Core Philosophy

The objective is to create a highly responsive network action game. To achieve this, **the client must never wait for the server to validate an action before displaying a response.**

- **Client Prediction:** The player hits a button, the player sees an immediate response.
- **Server Authority:** The client has zero simulation authority other than providing their inputs. The server remains the absolute source of truth to prevent cheating and resolve conflicts.
- **Mispredictions:** Disagreements between the client's predicted state and the server's authoritative state are handled gracefully via deterministic rollback and reconciliation, never at the expense of immediate responsiveness.

## 2. Simulation Fundamentals

Our deterministic simulation relies on a synchronized clock, fixed update intervals, and quantization.

- **Fixed Timestep (Command Frames):** Both client and server operate on quantized "Command Frames" (e.g., 16ms per frame for a 60Hz tick rate).
- **Accumulator Pattern:** The game loop translates variable render frames into fixed simulation ticks using an accumulator with rollover and remainder
- **ECS Integration:** Systems predicting on the client or simulating on the server do not use a variable `update()`. They use an `update_fixed()` step guaranteeing identical integration steps across both ends.

## 3. Time Synchronization & Client Lead

To minimize input delay on the server, the client's simulation clock always runs **ahead** of the server's clock.

- **Lead Formula:** `Client Time = Server Time + (RTT / 2) + 1 Buffered Command Frame`
- **Why?** The client gobbles up input as close to "now" as possible. By simulating ahead of the server by exactly the one-way trip time plus a tiny buffer, the client's input packets arrive at the server at the exact moment the server is ready to simulate that specific command frame.

## 4. Rollback and Reconciliation (Handling Mispredictions)

Because the client simulates ahead, it will occasionally mispredict (e.g., the client thought they turned in time to avoid a trail, but the server calculates they hit it).

- **Ring Buffers:** The client maintains two ring buffers:
  1. **Movement/State Buffer:** The history of the player's simulated states (positions, velocities, active abilities).
  2. **Input Buffer:** The history of the exact inputs (button presses, turns) submitted for each frame.
- **The Reconciliation Loop:**
  1. The client receives an authoritative snapshot from the server for a past tick (e.g., tick 17).
  2. The client checks if its local predicted state for tick 17 matches the server's state.
  3. If they agree, the client ignores the packet and continues.
  4. If they disagree (a misprediction), the client **overwrites** its local tick 17 state with the server's state.
  5. The client then **fast-forwards (replays)** all inputs from tick 18 up to the current predicted tick (e.g., tick 27) to catch back up to "now".

## 5. Network Resilience (Packet Loss & Jitter)

The game uses UDP, which is inherently lossy. We employ two major techniques to ensure simulation stability without sacrificing responsiveness:

### A. Sliding Window Inputs

- Instead of sending just the input for the current frame, the client sends a **sliding window of all inputs** starting from the last frame acknowledged by the server.
- _Example:_ If the server last acknowledged Frame 4, and the client just simulated Frame 19, the packet contains inputs for Frames 5 through 19.
- Since button states compress incredibly well (e.g., "Left Turn was held for 10 frames"), this payload is tiny but guarantees the server can fill in any dropped packets instantly once a subsequent packet arrives.

### B. Dynamic Time Dilation (Buffer Management)

- **Server Starvation:** If the server doesn't receive input in time for a frame, it duplicates the previous input, simulates it, and alerts the client.
- **Client Response (Dilation):** When the client hears the server is starved, it slightly speeds up its local simulation (e.g., ticking every 15.2ms instead of 16ms). This generates inputs slightly faster, inflating the server's input buffer to weather the packet loss/jitter.
- **Client Response (Contraction):** Once the server is healthy and has too large of a buffer, the client slows down its simulation clock (e.g., 16.8ms) to shrink the server's buffer back to the razor's edge, minimizing latency.

## 6. Resolution of Conflicts (Favor the Shooter vs. Mitigating Actions)

- **General Rule:** We favor the attacker/actor. If it looked like a valid kill/cutoff on the attacker's screen, the server will usually validate it.
- **The Exception (Evasive Abilities):** If the victim activated an evasive maneuver (e.g., a hypothetical "Shield" or "Jump" in Tron-Zero) on their client _before_ the attacker's input arrived at the server, the server honors the defensive ability, and the attacker misses.
