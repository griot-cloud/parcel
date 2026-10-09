---
layout: landing
content_max_width: 68rem
---

<div class="griot-home">

# parcel

<p class="home-lead">parcel is a data contract language and compiler for defining data quality rules and access policies, testing them against sample data, and compiling them for use in applications.</p>

<section class="home-usecases" aria-labelledby="usecases-title">
  <h2 id="usecases-title" class="home-section-title">Use cases</h2>
  <div class="home-usecase"><h3>Define how partners can use data</h3><p>Specify which orders each supplier may access, which columns are available, and which values need masking.</p></div>
  <div class="home-usecase"><h3>Check data before it reaches a report</h3><p>Require valid amounts, complete identifiers or fresh data. Choose whether a failed check excludes rows, blocks access or records a failure.</p></div>
  <div class="home-usecase"><h3>Test policy changes before using them</h3><p>Check a contract against sample data and caller profiles. Catch invalid expressions and see how many rows each caller would be allowed to read.</p></div>
  <div class="home-usecase"><h3>Use the same checks in another engine</h3><p>Export data validation as SQL or Substrait, or use parcel's Rust libraries in an application.</p></div>
</section>

<h2 class="home-section-title">Documentation</h2>
<nav class="home-cards" aria-label="Documentation sections">
  <a class="home-card" href="getting-started.html"><span class="card-number">01</span><h3>Getting started</h3><p>Install parcel, check sample data, and compile a contract.</p><span class="card-arrow" aria-hidden="true">↗</span></a>
  <a class="home-card" href="authoring.html"><span class="card-number">02</span><h3>Writing contracts</h3><p>Contract identity, binding, exposed columns, and rules.</p><span class="card-arrow" aria-hidden="true">↗</span></a>
  <a class="home-card" href="execution.html"><span class="card-number">03</span><h3>Checking and compiling contracts</h3><p>Validate data, test access policies, and save a compiled contract.</p><span class="card-arrow" aria-hidden="true">↗</span></a>
  <a class="home-card" href="reference.html"><span class="card-number">04</span><h3>Reference</h3><p>Contract fields, expressions, commands, and Rust libraries.</p><span class="card-arrow" aria-hidden="true">↗</span></a>
</nav>
<a class="home-next" href="getting-started.html"><span><small>Start here</small>Getting started</span><span aria-hidden="true">→</span></a>
</div>

```{toctree}
:maxdepth: 2
:hidden:

getting-started
authoring
execution
reference
```
