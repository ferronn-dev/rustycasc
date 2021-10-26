# rustycasc

A CASC parser and FrameXML extractor.

```
$ cargo install rustycasc
$ rustycasc
```

Writes .zip files containing the current FrameXML client files for each WoW
version into a `zips` subdirectory. Also writes intermediate state from the WoW
CDN in the `cache` directory.
