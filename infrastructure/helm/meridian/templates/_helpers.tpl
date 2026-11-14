{{/* Labels shared by every object. */}}
{{- define "meridian.labels" -}}
app.kubernetes.io/part-of: meridian
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/instance: {{ .Release.Name }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{/* Selector labels for one component. */}}
{{- define "meridian.selector" -}}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/name: {{ .name }}
{{- end -}}

{{/* Image reference for a component. */}}
{{- define "meridian.image" -}}
{{- $tag := required "image.tag is required" .root.Values.image.tag -}}
{{- printf "%s/%s:%s" .root.Values.image.registry .image $tag -}}
{{- end -}}

{{/* gRPC address of a service, in the release namespace. */}}
{{- define "meridian.grpcUrl" -}}
{{- printf "http://%s.%s.svc.cluster.local:9090" . $.Release.Namespace -}}
{{- end -}}

{{/* A service's settings merged over the defaults. */}}
{{- define "meridian.serviceValues" -}}
{{- toYaml (mergeOverwrite (deepCopy .root.Values.defaults) (deepCopy .svc)) -}}
{{- end -}}

{{/* Spread replicas across zones and nodes, best effort. */}}
{{- define "meridian.spread" -}}
topologySpreadConstraints:
  - maxSkew: 1
    topologyKey: topology.kubernetes.io/zone
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels: {{- include "meridian.selector" . | nindent 8 }}
  - maxSkew: 1
    topologyKey: kubernetes.io/hostname
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels: {{- include "meridian.selector" . | nindent 8 }}
{{- end -}}
