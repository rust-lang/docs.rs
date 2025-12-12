# validat

* [x] validate if some dependencies can bemoved from workspace to only crate deps
* [x] big binary crates should not depend on each other. the shared functionality should be extracte


## todo memaining
* [ ] re-add tests & write test helpers etc
* [ ] rewrite `build_queue_next_package` somehow nicer. Either just in the
   builder with some queue-lib help, or intelligently in the queue lib
