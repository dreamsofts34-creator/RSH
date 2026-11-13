# RSH — Code Signing (`docs/SIGNING.md`)

**Scope:** WP8 (Phase A) — how the RSH Windows app (MSIX, .NET 10) gets code-signed
and distributed. **Doc only.** Buying a certificate or creating any account is
the owner's job — this document tells them exactly what to buy, from whom, and
what to check first.

**Decisions this doc implements (locked, Plan v2.3):** D22 (MSIX primary),
D23 (signing). Plan reference: `RSH-PLAN-v2.3.md` §6 (Windows row) and §0 D22/D23.
Nothing in this doc overrides the plan.

**Product name is RSH.** Do not use any other product name in signing,
packaging, or Store listings.

---

## 1. Decision

| Item | Decision |
|---|---|
| Code-signing certificate | **OV (Organization Validation)** certificate, **$150–300/year** (plan figure — see §7, verify current price before buying) |
| Procurement | **Start in Phase A** — organization validation takes **days to weeks**, so it is on the critical path to the first signed release |
| Packaging | **MSIX is primary** (D22). WiX is dropped |
| Fallback distribution | **Microsoft Store MSIX** — registration fee waived since May 2026; the Store re-signs the package (see §4) |
| Azure Artifact Signing (formerly Trusted Signing) | **Closed route — do not pursue.** It is not available to a Pakistan-based individual developer; individuals are only eligible in the USA/Canada (D23) |

**Why OV, not anything else:** an OV certificate puts a verified organization
name in the signature, which is what Windows SmartScreen and users check.
There is no cheaper shortcut that gives the same publisher identity — and
there is no Azure-hosted signing option open to this project (see below).

**Why Azure Artifact Signing is closed (do not waste time on it):**
Azure Artifact Signing (previously called Trusted Signing) does **not**
accept a Pakistan-based individual developer — individual eligibility is
limited to the USA/Canada. This was research-verified for the plan (D23).
Do not open an Azure account, submit an application, or budget for it
expecting approval. If eligibility ever changes, that is a plan change
(D23 amendment), not something to assume during procurement.

**What signing does NOT do:** signing does **not** remove the SmartScreen
warning. Even an EV certificate has not given instant trust since 2024.
Trust ("reputation") is built over **weeks**, from download volume under one
consistent publisher identity (see §5). Budget for the first releases
showing a warning — that is normal, not a signing failure.

---

## 2. Getting an OV certificate

### 2.1 Choose a CA — what to compare

Buy from a publicly trusted Certificate Authority (CA) whose code-signing
root is in the Microsoft Trusted Root Program. Well-known examples to
compare (not an endorsement, and not an exhaustive list):

- **DigiCert**
- **Sectigo** (formerly Comodo CA)
- **GlobalSign**

Check before you pay — for each CA, confirm all of these:

1. **It is an OV code-signing certificate** (not DV — there is no DV code
   signing — and not an EV certificate unless you deliberately choose to
   pay more; EV is **not** required by this plan and does not buy instant
   SmartScreen trust).
2. **The certificate is usable for MSIX signing** — the CA must issue it for
   code signing (extended key usage: Code Signing) and it must be delivered
   in a form `signtool` can use (see §3).
3. **Current price and term** — the plan's figure is **$150–300/year**.
   Check the CA's own site for the current price before buying; do not rely
   on the plan figure or on reseller listings (see §7).
4. **Validation requirements and turnaround** — what documents they need
   and how long they take (see §2.2).
5. **Reissue / key-storage rules** — whether the private key is delivered as
   a file (PFX) or must live on a hardware token / HSM, and what that means
   for CI signing later (Phase H release workflow). Prefer a delivery method
   the release process can actually use; record the choice here when known.
6. **Renewal terms** — price on renewal (it is often not the first-year
   price), and how far in advance they remind you.

### 2.2 Organization validation — what you will need

OV means the CA verifies the **organization**, not just the applicant.
Validation takes **days to weeks** (plan figure) — this is the whole reason
procurement starts in Phase A. Expect the CA to ask for some or all of:

- Legal organization name, exactly as registered (this name becomes the
  publisher identity — see §5, it must never change between releases).
- Proof the organization exists: business registration / incorporation
  document, or equivalent official record.
- A verifiable business address (and phone number) — the CA checks it
  against public directories / official records.
- Proof you are authorized to request a certificate for the organization
  (e.g. a letter on company letterhead, or a callback to a publicly listed
  company number — the exact method is the CA's choice).
- A government-issued ID of the requester, if the CA asks for it.

**If the organization is not yet formally registered, stop here:** OV
validation cannot complete without a verifiable organization. Registering
the organization (or choosing the Store-only path, §4) is a prerequisite —
do not pay for the certificate first and discover this afterwards. The exact
documents depend on the CA; confirm the list with the chosen CA **before**
paying.

### 2.3 Cost and time (plan figures — verify before buying)

| Item | Plan figure |
|---|---|
| OV certificate | **$150–300/year** |
| Validation time | **Days to weeks** |
| Renewal | Yearly — set a reminder (see §2.4) |

Do not quote any other price in release planning. If a CA quotes outside
this range, that is fine — record the real quote, do not "correct" it to
fit the plan, and flag it to the team before paying.

### 2.4 Renewal reminder

- Put the **expiry date** in the calendar the day the certificate is issued,
  with reminders at **90, 30, and 7 days** before expiry.
- Start renewal at the **90-day** reminder — validation can be required again
  and also takes days to weeks.
- **Never let the certificate lapse between releases.** A release signed
  with an expired certificate fails validation; a release signed just before
  expiry stays valid only if it was timestamped (see §3).
- Record the expiry date and the renewal owner in the release checklist
  (Phase H) once the certificate exists.

---

## 3. Signing the MSIX (local signing with the OV certificate)

Do **not** use AzureSignTool — that tool is for the Azure Artifact Signing
service, which is closed to this project (§1). Sign locally with the OV
certificate using `signtool` (part of the Windows SDK).

### 3.1 The one rule that breaks builds: Publisher == certificate subject

The MSIX package manifest's `<Identity Publisher="...">` value must
**exactly match the Subject of the signing certificate** (D22). "Close
enough" does not work — a mismatch fails signing/validation. Before the
first signed build:

1. Read the certificate Subject (the organization name exactly as the CA
   issued it, e.g. `CN=<Organization Name>` — copy it character for
   character, do not retype from memory).
2. Put that exact string in the MSIX manifest `<Identity Publisher="...">`.
3. Add a CI/release check that compares the two and fails the build on
   mismatch (Phase H release workflow).

If the organization name ever changes (rebrand, re-registration), the
certificate must be reissued and the manifest updated together — and the
publisher identity consequences in §5 apply.

### 3.2 Signing steps (release build)

1. Build the MSIX package (unsigned) from the release build.
2. Sign with `signtool`, using the OV certificate from the Windows
   certificate store or a PFX file, and a digest algorithm the CA supports
   (SHA-256). Command shape (fill in the real values at release time):

   ```powershell
   signtool sign /fd SHA256 /a /tr <CA timestamp server URL> /td SHA256 <package>.msix
   ```

   - `/a` selects the best code-signing certificate in the store; use
     `/sha1 <thumbprint>` instead when several certificates are present,
     so the build never signs with the wrong one.
   - `/tr` + `/td` add an **RFC 3161 timestamp — always timestamp**.
     Timestamping proves the signature was made while the certificate was
     valid, so the package stays valid after the certificate expires.
     Use the timestamp server URL your CA documents; verify it before the
     first release build.
3. Verify the signature:

   ```powershell
   signtool verify /pa /v <package>.msix
   ```

   The build fails if verification fails — never ship an unverified package.
4. Record in the release notes: certificate thumbprint, expiry date, and
   timestamp server used.

### 3.3 Private-key handling (security — non-negotiable)

- The certificate's **private key is never committed to the repo**, never
  pasted into chat, docs, or tickets, and never emailed.
- Keep the PFX (if file-delivered) password-protected, in one controlled
  location, with access limited to the release owner. If the CA requires a
  hardware token / HSM, follow the CA's storage rules instead.
- CI signing (Phase H) must use the CI system's secret store — never a key
  file checked into the repository.
- If a private key is ever exposed or the token is lost, **revoke the
  certificate immediately** and reissue; do not keep shipping with it.

---

## 4. Microsoft Store path (fallback / parallel distribution)

The Store is the fallback if OV procurement stalls, and a parallel channel
even if it does not. It is a **fallback**, not a replacement for the OV
certificate on direct (non-Store) distribution — the Store re-signs only
the package it distributes.

### 4.1 Steps

1. **Create a Microsoft Partner Center developer account** (individual
   account is fine for this path). Registration fee has been **waived since
   May 2026** (plan figure — verify on the Partner Center site at sign-up;
   if a fee is shown, stop and check before paying).
2. **Reserve the app name "RSH"** in Partner Center (or the closest
   available name — record the final reserved name; it must not contain any
   other product name).
3. **Prepare the MSIX package** for submission (same package as §3, built
   from the same release build). The Store accepts the submission and
   **re-signs the package with Microsoft's certificate** on publish.
4. **Submit for certification** — the Store runs its certification checks
   (package validity, policy compliance). Fix what it flags; do not assume
   a first submission passes.
5. **Publish.** The Store listing is the distribution channel; users install
   from the Store, and updates come through the Store.

### 4.2 What the Store path buys you

- **No day-one SmartScreen warning** for the Store-installed package —
  because the Store re-signs it with Microsoft's certificate, the
  reputation problem in §5 does not apply to Store installs.
- A trusted install/update channel with no certificate procurement on the
  critical path.
- It does **not** remove the need for the OV certificate if RSH is also
  distributed outside the Store (direct download / winget). A direct-download
  MSIX signed with anything other than the OV certificate (or unsigned) is
  not an acceptable release.

### 4.3 Store gotchas to record at submission time

- The Store's re-signing means the Store package's publisher identity is
  Microsoft's, not yours — keep the direct-download and Store channels
  clearly separate in release notes so nobody mixes up which signature a
  given install carries.
- Store policy (naming, content, capabilities declared in the manifest)
  must be checked at submission; the manifest capabilities RSH needs
  (network, etc.) must be declared or certification fails.

---

## 5. SmartScreen + reputation plan

Read this before anyone promises a user "no warning":

1. **Signing does not remove the SmartScreen warning.** A signed package
   still shows a warning on first releases. Even an EV certificate has not
   given instant trust since 2024 (D23). Anyone claiming otherwise is wrong.
2. **Reputation is built over weeks from download volume** under one
   publisher identity. There is no purchase, setting, or form that speeds
   this up. Expect warnings on the first releases and plan support /
   documentation around them (tell users what the warning means and that
   the publisher name should read as the verified organization name).
3. **Publisher identity must be identical in every release — never rotate
   it.** Do not change the organization name, do not switch CAs in a way
   that changes the certificate Subject, and do not alternate between
   signing identities. A new identity resets reputation to zero.
4. **Do not do any of these:**
   - Do not buy a second certificate "to try" and sign some releases with
     it — that splits reputation across two identities.
   - Do not sign a release with a personal / self-signed certificate to
     "get something out" — it carries no trust and confuses the reputation
     picture.
   - Do not tell users to disable SmartScreen, click through blindly, or
     turn off security features. Document the warning honestly instead.
   - Do not rebrand or rename the publisher between releases.
5. **Store installs are the exception** (§4): the Store re-signs with
   Microsoft's certificate, so Store installs skip the day-one warning.
   This is the practical way to give early users a warning-free install
   while direct-download reputation builds.

---

## 6. Checklist — do this now (Phase A)

Owner actions — nothing here is done by the team writing this doc:

- [ ] **Start OV certificate procurement now** (WP8 / R-M28) — validation
      takes days to weeks; it is on the critical path to the first signed
      release.
- [ ] Decide the legal organization name that will appear as the publisher
      (it must never change afterwards — §5).
- [ ] Confirm the organization is formally registered / verifiable; if not,
      register it (or commit to the Store-only path, §4) before paying a CA.
- [ ] Compare CAs (DigiCert, Sectigo, GlobalSign — §2.1) and check the six
      items in §2.1, including **current price on the CA's own site**.
- [ ] Buy the OV certificate ($150–300/year per the plan — verify the real
      quote first).
- [ ] Complete organization validation; keep the CA's document requests and
      correspondence.
- [ ] On issue: record the certificate Subject (exact string), thumbprint,
      and expiry date; put the expiry in the calendar with 90/30/7-day
      reminders (§2.4).
- [ ] Set the MSIX manifest `<Identity Publisher="...">` to the exact
      certificate Subject (§3.1).
- [ ] Create the Microsoft Partner Center developer account (fee waived
      since May 2026 — verify at sign-up) and reserve the app name (§4).
- [ ] Do a first signed test build (`signtool sign` + `verify`, §3.2) before
      the Phase H release — do not discover a Publisher mismatch on release
      day.

---

## 7. What this doc could NOT verify — check before you pay

These items are **not** verified by this doc and must be checked on the
source named, before money is spent:

| Item | Status | Check at |
|---|---|---|
| OV certificate current price | Plan figure is **$150–300/year**; the real price may differ | The chosen CA's own website, before buying |
| CA validation document list & turnaround | Plan figure is "days to weeks"; the exact list is the CA's | The chosen CA (confirm before paying) |
| Azure Artifact Signing eligibility | Plan states it is **not available** to a Pakistan-based individual (individuals USA/Canada only) — taken from the plan, not re-verified here | Microsoft's Trusted Signing / Artifact Signing eligibility page, if you ever revisit D23 |
| Microsoft Store registration fee | Plan states **waived since May 2026** — taken from the plan, not re-verified here | Partner Center sign-up page, at account creation |
| SmartScreen / EV trust behaviour | Plan states EV has given no instant trust since 2024 — taken from the plan, not re-verified here | Microsoft SmartScreen documentation, if the claim is ever challenged |
| Timestamp server URL | Not stated here on purpose — it is CA-specific | Your CA's documentation, before the first signed build |
| Any CA not named in §2.1 | Not evaluated | That CA's site + Microsoft Trusted Root Program membership |

**Rule for the whole doc:** the only prices and eligibility claims in here
are the plan's (D23 / §6). Anything else is marked "verify before buying"
above. If a source contradicts the plan, do not silently edit this doc —
raise it as a plan change (D23 amendment).

---

*Sources: `RSH-PLAN-v2.3.md` §0 D22/D23, §5 Phase A (WP8 / R-M28), §6
(Windows row), §7 (risk: signing eligibility). Windows 11 is the floor
(Windows 10 EOL 14 Oct 2025); .NET 10 LTS is supported to 14 Nov 2028.*
