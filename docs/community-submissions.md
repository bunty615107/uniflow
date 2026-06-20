# UniFlow Community Submissions & Outreach Templates

This document contains templates and submission information for various community aggregators, subreddits, and developer blogs to help spread the word about UniFlow.

## Awesome Lists Submissions

### 1. Awesome Rust

**Target:** [rust-unofficial/awesome-rust](https://github.com/rust-unofficial/awesome-rust)

**Category:** Applications / Network

**Submission Line:**
```markdown
* [uniflow](https://github.com/example/uniflow) - A secure, connection-agnostic managed file transfer (MFT) system with P2P, Cloud, and Local delta-sync capabilities, featuring a retro-futurist web UI.
```

**PR Template (awesome-rust):**
```markdown
### What does this PR do?
Adds `uniflow` to the Applications / Network category.

### Why should it be added?
UniFlow is a production-grade Managed File Transfer daemon built in Rust. It provides a unique orchestration layer across different transports (P2P via iroh/QUIC, Local deltas via BLAKE3/librsync, and Cloud via rclone) with baked-in client-side encryption and a tamper-evident audit log. It also includes an embedded Axum-based web interface.

### Checklist
- [x] I have read the [contributing guidelines](https://github.com/rust-unofficial/awesome-rust/blob/master/CONTRIBUTING.md).
- [x] The project is well-documented and has a clear README.
- [x] The project uses Rust and fits the category.
```

### 2. Awesome Rust Web

**Target:** [awesome-rust-web](https://github.com/rust-unofficial/awesome-rust-web) (or similar web-focused rust lists)

**Category:** Fullstack Applications / Web Servers

**Submission Line:**
```markdown
* [uniflow](https://github.com/example/uniflow) - MFT daemon with an embedded Axum web server providing a complete REST API and live-binding retro-futurist web UI.
```

**PR Template (awesome-rust-web):**
```markdown
### What does this PR do?
Adds `uniflow` to the Fullstack Applications section.

### Why should it be added?
UniFlow demonstrates a clean architecture approach to embedding a sophisticated web application (Axum + Tower + Askama/Static) within a background daemon. It includes live data binding, rate limiting, and robust API security, serving as a strong example of Rust in full-stack network applications.
```

---

## Reddit Launch Templates

### 1. r/rust

**Title:** [Project] UniFlow - A secure, connection-agnostic Managed File Transfer (MFT) daemon with a retro web UI

**Body:**
> Hey r/rust,
>
> I wanted to share a project I've been working on called **UniFlow**. It’s a Managed File Transfer (MFT) system built from the ground up in Rust to unify file movement across local disks, cloud providers, and P2P networks (using `iroh` and QUIC).
>
> **Why I built it:**
> Traditional MFT tools are often siloed (one tool for S3, another for SFTP, another for P2P) and bolt on security as an afterthought. I wanted a single, secure control plane with a unified job model.
>
> **Key features:**
> * **Pluggable Transports:** Works across Local Deltas (BLAKE3 + librsync), Cloud (via rclone), and P2P (iroh).
> * **Baked-in Security:** Client-side encryption (AES-GCM/ChaCha20), tamper-evident BLAKE3 audit trails.
> * **Clean Architecture:** Strict separation between Domain, Application, and Infrastructure layers.
> * **Embedded Web UI:** Serves a retro-futurist "Stitch" UI directly via `axum`, bound to a real REST API.
>
> The codebase is heavily focused on security and clean architecture (traits for transports, persistence, and intelligence engines). I’d love for people to check out the code, especially the daemon orchestration and Axum integration.
>
> **Repo:** [https://github.com/example/uniflow](https://github.com/example/uniflow)
>
> Feedback, code reviews, and PRs are extremely welcome!

### 2. r/webdev

**Title:** Show r/webdev: Serving a retro-futurist web app from a Rust backend (UniFlow)

**Body:**
> Hi r/webdev,
>
> Usually, backend daemons have pretty boring or non-existent UIs. For my latest project, **UniFlow** (a secure file transfer daemon), I wanted to embed a first-class web experience directly into the binary.
>
> I ended up building a "retro-futurist" inspired interface (think high-end editorial design meets classic sci-fi terminals).
>
> **How it works:**
> * The backend is a Rust daemon running `axum`.
> * It serves statically bundled HTML/JS/CSS prototypes.
> * The frontend uses vanilla JS to dynamically bind to the daemon's REST API.
> * It features live kanban boards for job states, real-time audit feeds, and a job builder.
>
> I spent a lot of time ensuring the API is secure (rate limiting, XSS prevention, path traversal protection). It’s been a fun experiment in shipping a zero-dependency (from the user's perspective) full-stack app.
>
> Check out the screenshots in the repo or run it locally!
>
> **Repo:** [https://github.com/example/uniflow](https://github.com/example/uniflow)

### 3. r/opensource

**Title:** UniFlow: An open-source, secure universal file transfer orchestrator

**Body:**
> Hello r/opensource,
>
> I'm excited to share **UniFlow**, a modern Managed File Transfer (MFT) platform I've open-sourced.
>
> UniFlow acts as a universal transfer bus. Instead of juggling different tools for cloud storage, local network sync, or secure peer-to-peer sharing, you define a `Job` (Source, Destination, Policy) and UniFlow routes it to the best transport.
>
> **Highlights:**
> * Written in Rust for safety and performance.
> * Connection-agnostic: Supports local deltas, P2P (QUIC), and Cloud.
> * Zero-knowledge ready: Client-side encryption and tamper-evident audit logs.
> * Ships with a really cool embedded web interface.
>
> We are looking for contributors who are interested in network programming, security, or frontend development. Check out the repository if you're looking for a new project to dive into!
>
> **Repo:** [https://github.com/example/uniflow](https://github.com/example/uniflow)

---

## Dev.to Blog Post Outline

**Title:** Building UniFlow: A Connection-Agnostic File Transfer System in Rust

**Introduction**
* The problem with file transfer today: fragmented tools (rsync, scp, rclone, custom scripts).
* The lack of built-in, verifiable security and auditability in traditional tools.
* Introducing UniFlow: The unified control plane.

**1. The Architecture: Clean and Modular**
* Exploring the Domain / Application / Infrastructure split.
* How the `Transport` trait allows swapping local sync, cloud, and P2P without touching the core logic.
* Managing state with a robust Job Status machine.

**2. Transport Deep Dive: P2P and Deltas**
* How we use `iroh` and QUIC for NAT traversal and direct peer-to-peer transfers.
* The local delta engine: Combining `BLAKE3` for blazing fast hashing and `librsync` for rolling checksums.

**3. Security is not an Afterthought**
* Implementing the tamper-evident audit log (hash chaining with BLAKE3).
* Enforcing zero-knowledge encryption (AES-256-GCM / ChaCha20-Poly1305) on the client side before data hits the transport layer.

**4. The Retro-Futurist Web UI**
* Why an embedded UI? The power of single-binary deployment.
* Building a secure, dynamic UI with `axum` and vanilla JS bindings.
* Hardening the API against XSS, path traversal, and unauthorized access.

**Conclusion**
* What's next for UniFlow (RocksDB persistence, intelligent auto-tuning).
* Call to action: Check out the repo, star it, and contribute!
* Link to GitHub repository.
