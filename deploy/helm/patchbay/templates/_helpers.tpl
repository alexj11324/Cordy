{{/*
Common labels for all resources.
*/}}
{{- define "patchbay.labels" -}}
app.kubernetes.io/name: patchbay
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{/*
Deployment selectors are immutable. This compatibility label intentionally
keeps the pre-brand value so existing releases can be upgraded in place.
Patchbay remains the metadata/part-of brand on every rendered workload.
*/}}
{{- define "patchbay.selectorLabels" -}}
app.kubernetes.io/name: cordy # legacy-brand-compat
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "patchbay.podLabels" -}}
{{ include "patchbay.selectorLabels" . }}
app.kubernetes.io/part-of: patchbay
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{/*
Per-component resource names. Using Release.Name keeps the same name we used
under the kustomize layout when installed as `helm install patchbay ...`.
*/}}
{{- define "patchbay.backend.fullname" -}}
{{ .Release.Name }}-backend
{{- end -}}

{{- define "patchbay.frontend.fullname" -}}
{{ .Release.Name }}-frontend
{{- end -}}

{{- define "patchbay.postgres.fullname" -}}
{{ .Release.Name }}-postgres
{{- end -}}

{{/*
DATABASE_URL pieced together from the postgres service + Secret values.
The $(VAR) syntax is resolved by the kubelet from the container's env, so
POSTGRES_USER / POSTGRES_PASSWORD / POSTGRES_DB must also be loaded into env
on the same container (see envFrom on the backend Deployment).
*/}}
{{- define "patchbay.databaseUrl" -}}
postgres://$(POSTGRES_USER):$(POSTGRES_PASSWORD)@{{ include "patchbay.postgres.fullname" . }}:5432/$(POSTGRES_DB)?sslmode=disable
{{- end -}}

{{/*
Prefer the Patchbay Secret on new installs. During an in-place upgrade, fall
back to the external Secret used by the previous chart when the new Secret has
not been created. Users can still select any explicit Secret name.
*/}}
{{- define "patchbay.existingSecret" -}}
{{- $preferred := .Values.existingSecret -}}
{{- $preferredObject := lookup "v1" "Secret" .Release.Namespace $preferred -}}
{{- $legacyName := "cordy-secrets" -}} {{/* legacy-brand-compat */}}
{{- $legacyObject := lookup "v1" "Secret" .Release.Namespace $legacyName -}}
{{- if or $preferredObject (ne $preferred "patchbay-secrets") -}}
{{- $preferred -}}
{{- else if $legacyObject -}}
{{- $legacyName -}}
{{- else -}}
{{- $preferred -}}
{{- end -}}
{{- end -}}

{{/*
PostgreSQL initialization variables do not rename a database or role on an
existing PVC. Preserve the live ConfigMap identity by default on upgrades;
fresh installs render the Patchbay values from values.yaml.
*/}}
{{- define "patchbay.postgresDatabase" -}}
{{- $existing := lookup "v1" "ConfigMap" .Release.Namespace (printf "%s-config" .Release.Name) -}}
{{- if and .Values.postgres.preserveExistingIdentity $existing $existing.data (index $existing.data "POSTGRES_DB") -}}
{{- index $existing.data "POSTGRES_DB" -}}
{{- else -}}
{{- .Values.postgres.database -}}
{{- end -}}
{{- end -}}

{{- define "patchbay.postgresUser" -}}
{{- $existing := lookup "v1" "ConfigMap" .Release.Namespace (printf "%s-config" .Release.Name) -}}
{{- if and .Values.postgres.preserveExistingIdentity $existing $existing.data (index $existing.data "POSTGRES_USER") -}}
{{- index $existing.data "POSTGRES_USER" -}}
{{- else -}}
{{- .Values.postgres.user -}}
{{- end -}}
{{- end -}}
