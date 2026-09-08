{{- define "orvilo-auth-broker.fullname" -}}{{ printf "%s-auth-broker" .Release.Name | trunc 63 | trimSuffix "-" }}{{- end }}
{{- define "orvilo-auth-broker.labels" -}}app.kubernetes.io/name: orvilo-auth-broker
app.kubernetes.io/instance: {{ .Release.Name }}{{- end }}
