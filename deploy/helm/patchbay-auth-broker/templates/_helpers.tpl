{{- define "patchbay-auth-broker.fullname" -}}
{{- printf "%s-auth-broker" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "patchbay-auth-broker.labels" -}}
app.kubernetes.io/name: patchbay-auth-broker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: patchbay
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
{{- end -}}

{{- define "patchbay-auth-broker.selectorLabels" -}}
app.kubernetes.io/name: patchbay-auth-broker
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}
