#!/bin/sh
exec "/home/kaalin/dev/gnata/crates/gnata-rs/target/release/gnata-bench" -expr 'Account.Name & " Corp"' -datafile "/home/kaalin/dev/gnata/bench/data.json" -n 20000
