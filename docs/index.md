# rustycasc

## Product names

| Product Name              | Product Tag           |
| ------------------------- | --------------------- |
| Retail                    | `wow`                 |
| Retail PTR                | `wowt`                |
| Classic (TBC)             | `wow_classic`         |
| Classic (TBC) PTR         | `wow_classic_ptr`     |
| Classic Era (Vanilla)     | `wow_classic_era`     |
| Classic Era (Vanilla) PTR | `wow_classic_era_ptr` |

## How content is retrieved

*   patch version points to build config and cdn config
*   cdn config points to archives
*   build config points to root and encoding
*   root maps filedataids to content keys
*   encoding maps content keys to encoding keys
*   archive indexes map encoding keys to archive byte ranges
