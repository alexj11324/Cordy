{{- define "patchbay-auth-broker.fullname" -}}{{ printf "%s-auth-broker" .Release.Name | trunc 63 | trimSuffix "-" }}{{- end }}
{{- define "patchbay-auth-broker.labels" -}}app.kubernetes.io/name: patchbay-auth-broker
app.kubernetes.io/instance: {{ .Release.Name }}{{- end }}
