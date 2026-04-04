#!/bin/sh
exec "/home/kaalin/dev/gnata/bench/go_bench" -expr 'Account.Name & " Corp"' -datafile "/home/kaalin/dev/gnata/bench/data.json" -n 20000
