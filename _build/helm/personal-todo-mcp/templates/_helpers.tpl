{{- define "personal-todo-mcp.selectorLabels" -}}
app.kubernetes.io/name: personal-todo-mcp
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "personal-todo-mcp.labels" -}}
{{ include "personal-todo-mcp.selectorLabels" . }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end }}

{{/* The rmcp Host allowlist comes from this URL, so it must not be empty. */}}
{{- define "personal-todo-mcp.host" -}}
{{- $url := required "publicBaseUrl is required" .Values.publicBaseUrl -}}
{{- (urlParse $url).host -}}
{{- end }}
