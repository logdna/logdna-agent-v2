# Mangling Line Metadata  

You can use `LOGDNA_META_XXXXX` environment variables to override or omit various metadata fields before they get sent to server. 
Empty value will cause a field to be omitted or removed.

Notes:
- LOGDNA_META_ANNOTATIONS & LOGDNA_META_LABELS variables expect key-value-pairs values in JSON format.
- LOGDNA_META_JSON variable expects any valid JSON value.
- LOGDNA_META_FILE - gets processed before LOGDNA_META_K8S_FILE.


## Example HOST

Override host value to hide sensitive information

```yaml
    - env:
        - name: LOGDNA_META_HOST
          value: REDACTED_HOST
```

## Example LABELS

Add new label or override existing one

```yaml
    - env:
        - name: LOGDNA_META_LABELS
          value: "{\"my_label_name\":\"my_label_value\"}"
```

Remove specific label from label LABELS list

```yaml
    - env:
        - name: LOGDNA_META_LABELS
          value: "{\"my_label_name\":\"\"}"
```
