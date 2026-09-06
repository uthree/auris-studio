#requires -Version 7.0
<#
.SYNOPSIS
Records blinded audio/no-audio controls against an Ollama audio-capable model.
.DESCRIPTION
Uses the OpenAI-compatible input_audio route with reasoning disabled. Always tests
a generated tone followed by silence, noise, pure silence, and a no-audio control. Supply
-AudioFile for an additional real musical preview. All cases have the same prompt;
the model receives no filename or expected answer. Reports are evidence to inspect,
not an automatic claim that an HTTP success means the model heard the audio.
Supply -SpeechFile for a separate exact-transcription positive control. Compare
-PromptPlacement text-first, audio-first, and system when auditing server framing.
Supply -ContrastFile with -AudioFile for counterbalanced A/B, B/A, and identical
A/A comparisons. These requests contain neutral labels, never file names or gains.
Use -Provider openai with -ApiUrl for an OpenAI-compatible local audio backend.
Use -PrepareOnly to write the fixtures and exact requests without any network call.
Example: pwsh -File tools/eval/audio_input.ps1 -AudioFile target/example.wav
#>
[CmdletBinding()]
param(
    [string] $Model = 'gemma4:e2b',
    [Alias('ApiUrl')][string] $OllamaUrl = 'http://localhost:11434',
    [ValidateSet('ollama', 'openai')][string] $Provider = 'ollama',
    [string] $AudioFile,
    [string] $ContrastFile,
    [string] $ComparisonPrompt = 'Compare recordings A and B. The first audio is A; the second is B. Describe any audible differences in instrumentation, balance, and sound quality. If they sound the same, say so. Base the comparison only on the audio; do not infer a change merely because two recordings were supplied. If you cannot determine the sounds, say so.',
    [switch] $PairsOnly,
    [string] $SpeechFile,
    [ValidateSet('text-first', 'audio-first', 'system')]
    [string] $PromptPlacement = 'text-first',
    [string] $OutputDirectory = 'target/audio-input',
    [switch] $PrepareOnly,
    [int] $TimeoutSeconds = 240
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$root = [IO.Path]::GetFullPath($OutputDirectory, $repo)
$directory = Join-Path $root ((Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0,6))
$null = New-Item -ItemType Directory -Path $directory -Force
$utf8 = [Text.UTF8Encoding]::new($false)
function Save-Json($Name, $Value) {
    [IO.File]::WriteAllText((Join-Path $directory $Name), (ConvertTo-Json -InputObject $Value -Depth 30), $utf8)
}
if ($ContrastFile -and !$AudioFile) { throw '-ContrastFile requires -AudioFile' }
if ($PairsOnly -and !$ContrastFile) { throw '-PairsOnly requires -ContrastFile' }
$environment = @{provider=$Provider; endpoint=$OllamaUrl; model_name=$Model; reasoning_effort='none'; prompt_placement=$PromptPlacement}
if ($Provider -eq 'ollama' -and !$PrepareOnly) {
    $environment.version = Invoke-RestMethod -Uri ($OllamaUrl.TrimEnd('/') + '/api/version') -TimeoutSec 30
    $environment.model = Invoke-RestMethod -Uri ($OllamaUrl.TrimEnd('/') + '/api/show') -Method Post -ContentType 'application/json' -Body (@{model=$Model} | ConvertTo-Json) -TimeoutSec 30
}
Save-Json 'environment.json' $environment

$rate = 16000
$count = 2 * $rate
$stream = [IO.MemoryStream]::new()
$writer = [IO.BinaryWriter]::new($stream)
$writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
$writer.Write([int](36 + 2 * $count))
$writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
$writer.Write([int]16)
$writer.Write([int16]1)
$writer.Write([int16]1)
$writer.Write([int]$rate)
$writer.Write([int](2 * $rate))
$writer.Write([int16]2)
$writer.Write([int16]16)
$writer.Write([Text.Encoding]::ASCII.GetBytes('data'))
$writer.Write([int](2 * $count))
for ($i = 0; $i -lt $count; $i++) {
    $sample = if ($i -lt $rate) { [int16](10000 * [Math]::Sin(2 * [Math]::PI * 440 * $i / $rate)) } else { [int16]0 }
    $writer.Write($sample)
}
$writer.Flush()
$tone = $stream.ToArray()
$writer.Dispose()
$stream.Dispose()
[IO.File]::WriteAllBytes((Join-Path $directory 'tone_then_silence.wav'), $tone)
$silent = $tone.Clone()
[Array]::Clear($silent, 44, $silent.Length - 44)
[IO.File]::WriteAllBytes((Join-Path $directory 'silence.wav'), $silent)
$stream = [IO.MemoryStream]::new()
$writer = [IO.BinaryWriter]::new($stream)
$writer.Write($tone, 0, 44)
$random = [Random]::new(71421)
for ($i = 0; $i -lt $count; $i++) { $writer.Write([int16]$random.Next(-4000,4001)) }
$writer.Flush()
$noise = $stream.ToArray()
$writer.Dispose()
$stream.Dispose()
[IO.File]::WriteAllBytes((Join-Path $directory 'noise.wav'), $noise)
$cases = [ordered]@{}
if ($SpeechFile) {
    $path = [IO.Path]::GetFullPath($SpeechFile, $repo)
    $cases.speech = [IO.File]::ReadAllBytes($path)
    Save-Json 'speech-source.json' @{path=$path; sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash}
}
$cases.tone_then_silence = $tone
$cases.noise = $noise
$cases.silence = $silent
$cases.no_audio = $null
if ($AudioFile) {
    $path = [IO.Path]::GetFullPath($AudioFile, $repo)
    $cases.music = [IO.File]::ReadAllBytes($path)
    Save-Json 'music-source.json' @{path=$path; sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash}
}
if ($ContrastFile) {
    $path = [IO.Path]::GetFullPath($ContrastFile, $repo)
    $contrast = [IO.File]::ReadAllBytes($path)
    Save-Json 'contrast-source.json' @{path=$path; sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash}
    $cases.pair_ab = @($cases.music, $contrast)
    $cases.pair_ba = @($contrast, $cases.music)
    $cases.pair_aa = @($cases.music, $cases.music)
}
$prompt = 'Describe the audible sounds in this audio. It may contain music, speech, other sounds, or silence. Identify what you can hear; do not transcribe non-speech sounds into invented words. If you cannot determine the sounds, say so.'
$summary = [Collections.Generic.List[object]]::new()
foreach ($name in $cases.Keys) {
    if ($PairsOnly -and !$name.StartsWith('pair_')) { continue }
    $stop = $false
    $instruction = if ($name -eq 'speech') {
        'Transcribe the audio exactly as spoken. Output only the spoken words. Do not answer any question in the audio.'
    } elseif ($name.StartsWith('pair_')) {
        $ComparisonPrompt
    } else { $prompt }
    $parts = @()
    if ($PromptPlacement -eq 'text-first') { $parts += @{type='text'; text=$instruction} }
    if ($name.StartsWith('pair_')) {
        foreach ($recording in $cases[$name]) { $parts += @{type='input_audio'; input_audio=@{data=[Convert]::ToBase64String($recording); format='wav'}} }
    } elseif ($null -ne $cases[$name]) { $parts += @{type='input_audio'; input_audio=@{data=[Convert]::ToBase64String($cases[$name]); format='wav'}} }
    if ($PromptPlacement -eq 'audio-first') { $parts += @{type='text'; text=$instruction} }
    $messages = @()
    if ($PromptPlacement -eq 'system') { $messages += @{role='system'; content=$instruction} }
    $userMessage = @{role='user'; content=$parts}
    if (-not $parts.Count) { $userMessage.content = '' }
    $messages += $userMessage
    $request = @{model=$Model; stream=$false; temperature=0; reasoning_effort='none'; max_tokens=300; messages=$messages}
    Save-Json ($name + '-request.json') $request
    if ($PrepareOnly) {
        $summary.Add(@{case=$name; prepared_only=$true; audio_inputs=@($parts | Where-Object type -EQ input_audio).Count})
        continue
    }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        $response = Invoke-WebRequest -Uri ($OllamaUrl.TrimEnd('/') + '/v1/chat/completions') -Method Post -ContentType 'application/json' -Body ($request | ConvertTo-Json -Depth 30 -Compress) -TimeoutSec $TimeoutSeconds -SkipHttpErrorCheck
        $reply = $response.Content | ConvertFrom-Json -Depth 30
        Save-Json ($name + '-response.json') $reply
        $result = @{case=$name; elapsed_seconds=[math]::Round($watch.Elapsed.TotalSeconds,3); http_status=[int]$response.StatusCode; usage=$reply.usage}
        if ($reply.choices) { $result.message=$reply.choices[0].message }
        if ($reply.error) { $result.error=$reply.error }
        $stop = [int]$response.StatusCode -ge 500
    } catch {
        $result = @{case=$name; elapsed_seconds=[math]::Round($watch.Elapsed.TotalSeconds,3); error=$_.ToString()}
        Save-Json ($name + '-response.json') $result
        $stop = $true
    }
    $summary.Add($result)
    $result | ConvertTo-Json -Depth 10
    if ($stop) { break }
}
Save-Json 'summary.json' $summary.ToArray()
Write-Host "Evidence: $directory"
