#requires -Version 7.0
<#
.SYNOPSIS
Tests local model tool use through live MCP stdio and the actual rig agent.
.DESCRIPTION
Build auris-mcp and auris-agent first. Runs are sequential and create new projects
under target/agent-tools. No model, server, or saved application setting is changed.
Pass -Transport control for a deterministic MCP fixture/readback check without a model.
Each run records the prompt, full wire events, binary hashes, and persisted-state verdict.
Example: pwsh -File tools/eval/agent_tools.ps1 -BinaryDirectory target/debug
#>
[CmdletBinding()]
param(
    [string] $BinaryDirectory = 'target/debug',
    [string] $OutputDirectory = 'target/agent-tools',
    [string[]] $Models = @('ornith-1.5:9b'),
    [ValidateSet('mcp', 'rig', 'control')][string[]] $Transport = @('mcp', 'rig'),
    [ValidateSet('editing', 'production', 'listening', 'critique')][string[]] $Scenario = @('editing', 'production'),
    [ValidateSet('en', 'ja')][string] $PromptLanguage = 'en',
    [string] $OllamaUrl = 'http://localhost:11434',
    [int] $ContextTokens = 32768,
    [int] $MaxTurns = 16,
    [int] $TimeoutSeconds = 600
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$binaries = [IO.Path]::GetFullPath($BinaryDirectory, $repo)
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory, $repo)
$suffix = if ($IsWindows) { '.exe' } else { '' }
$mcpBinary = Join-Path $binaries "auris-mcp$suffix"
$agentBinary = Join-Path $binaries "auris-agent$suffix"
foreach ($binary in @($mcpBinary, $agentBinary)) {
    if (!(Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Build the binaries first: missing $binary" }
}
$sessionDirectory = Join-Path $outputRoot ((Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 6))
$null = New-Item -ItemType Directory -Path $sessionDirectory -Force
$utf8 = [Text.UTF8Encoding]::new($false)

function Write-Json($Path, $Value) {
    [IO.File]::WriteAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 100), $utf8)
}

function Add-Event($Path, $Value) {
    $Value.at_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::AppendAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 100 -Compress) + "`n", $utf8)
}

function Start-Wire($Binary, $Directory, $Arguments = @()) {
    $info = [Diagnostics.ProcessStartInfo]::new($Binary)
    $info.WorkingDirectory = $Directory
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardInputEncoding = $utf8
    $info.StandardOutputEncoding = $utf8
    $info.StandardErrorEncoding = $utf8
    $info.Environment.Remove('AURIS_AGENT_HISTORY') | Out-Null
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($info)
    return @{ process=$process; stderr=$process.StandardError.ReadToEndAsync(); id=0; log=(Join-Path $Directory ([IO.Path]::GetFileNameWithoutExtension($Binary) + '.jsonl')) }
}

function Stop-Wire($Wire) {
    if (!$Wire) { return }
    $Wire.process.StandardInput.Close()
    if (!$Wire.process.WaitForExit(3000)) { $Wire.process.Kill($true); $Wire.process.WaitForExit() }
    [IO.File]::WriteAllText(($Wire.log + '.stderr.txt'), $Wire.stderr.GetAwaiter().GetResult(), $utf8)
    $Wire.process.Dispose()
}

function Send-Wire($Wire, $Value) {
    Add-Event $Wire.log @{ direction='request'; payload=$Value }
    $Wire.process.StandardInput.WriteLine((ConvertTo-Json -InputObject $Value -Depth 100 -Compress))
    $Wire.process.StandardInput.Flush()
}

function Read-Wire($Wire) {
    $read = $Wire.process.StandardOutput.ReadLineAsync()
    if (!$read.Wait($TimeoutSeconds * 1000)) { throw "Timed out waiting for $($Wire.log)" }
    $line = $read.GetAwaiter().GetResult()
    if ($null -eq $line) { throw "Process closed stdout; inspect $($Wire.log).stderr.txt" }
    $reply = ConvertFrom-Json -InputObject $line -AsHashtable -Depth 100
    Add-Event $Wire.log @{ direction='response'; payload=$reply }
    return $reply
}

function Invoke-Mcp($Wire, $Method, $Parameters) {
    $Wire.id++
    $id = $Wire.id
    Send-Wire $Wire @{ jsonrpc='2.0'; id=$id; method=$Method; params=$Parameters }
    do { $reply = Read-Wire $Wire } while ($reply.id -ne $id)
    if ($reply.error) { throw ($reply.error | ConvertTo-Json -Compress) }
    return $reply.result
}

function Invoke-Tool($Wire, $Name, $Arguments) {
    return Invoke-Mcp $Wire 'tools/call' @{ name=$Name; arguments=$Arguments }
}

function Assert-Tool($Result) {
    if ($Result.isError) { throw ($Result.content | ConvertTo-Json -Compress) }
}

function Initialize-Mcp($Wire) {
    $info = Invoke-Mcp $Wire 'initialize' @{ protocolVersion='2024-11-05'; capabilities=@{}; clientInfo=@{name='auris-local-model-smoke'; version='1'} }
    Send-Wire $Wire @{ jsonrpc='2.0'; method='notifications/initialized' }
    return $info
}

function New-Fixture($Wire, $Directory, $Case) {
    $spec = @'
title = "Local Tool Smoke"
key = "C major"
tempo = 120
meter = "4/4"
seed = 71421
chords = "| I | V |"
form = ["verse"]
[section.verse]
bars = 2
parts = "lead bass"
[[part]]
name = "lead"
instrument = "auris.synth.chiptune"
[[part]]
name = "bass"
instrument = "auris.synth.fm2"
'@
    $project = Join-Path $Directory 'Smoke/Smoke.auris'
    Assert-Tool (Invoke-Tool $Wire 'compose' @{spec=$spec; output=(Join-Path $Directory 'Smoke.auris')})
    if (!(Test-Path -LiteralPath $project)) { throw "compose did not persist $project" }
    # Avoid testing arithmetic on the rounded display of the composer's normalization gain.
    if ($Case -in @('listening','critique')) { Assert-Tool (Invoke-Tool $Wire 'set_level' @{project=$project; track='lead'; gain_db=0}) }
    if ($Case -eq 'critique') { Assert-Tool (Invoke-Tool $Wire 'set_level' @{project=$project; track='bass'; gain_db=-12}) }
    return $project
}

function Get-Prompt($Project, $Case) {
    if ($PromptLanguage -eq 'ja') {
        $instruction = switch ($Case) {
            'editing' { 'lead トラックを TrialLead に改名し、音量を -7.5 dB、パンを -0.25 に設定してください。TrialBus というバストラックを追加してください。TrialLead の5小節目から2小節分、Manual という空のクリップを作り、5小節目の1・2・3拍目にそれぞれ C4・E4・G4 を追加してください。各音の長さは0.5拍、ベロシティは0.8です。元の生成済みクリップと bass は変更せず、保存した結果を読み返して確認してください。' }
            'production' { 'lead トラックにコンプレッサーを1つ追加し、bass トラックをそのサイドチェーン入力につないでください。コンプレッサーのスレッショルドは -24 dB にしてください。lead の音量に、曲頭から数えた四分音符0拍目の -12 dB から8拍目の -3 dB まで直線で変わるオートメーションを設定してください。既存の音符は変更しないでください。実際に使えるエフェクトIDとパラメーター名を調べ、保存後に結果を読み返してください。' }
            'listening' { 'listen で1小節目の1小節分を実際に試聴し、音声の講評を取得してください。その後 lead の現在のフェーダー値を読み、そこから正確に6 dB下げて保存し、listen で同じ範囲をもう一度試聴してください。2回目は、1回目の結果の audio_path を compare_to に指定して前後の音声を渡してください。前後2回の実際の講評を比べて報告してください。音符は変更しないでください。音声を判別できない、または聴取できないという回答の場合はその事実を明示し、音についての感想を作らないでください。その場合も指定の音量変更と2回目の試聴は実行してください。' }
            'critique' { 'listen で1小節目の1小節分を実際に試聴し、楽器のバランスについて中立的な講評を取得してください。講評に基づき、改善のためにトラック1本のフェーダーだけを調整してください。対象と変更量は講評から判断し、選んだ理由を説明してください。保存した値を読み返し、同じ範囲をもう一度 listen で試聴してください。2回目は1回目の audio_path を compare_to に指定し、実際の講評を比べてください。音符や他の設定は変更しないでください。講評が音を判別できない場合や変更を裏付けない場合は、理由を明示して変更を控えてください。聴いていない感想や改善の成功を作らないでください。' }
        }
        return "次の既存プロジェクトを編集してください: $Project`n$instruction`n利用可能なツールで今すぐ作業してください。書き込み先はこのプロジェクト内に限ります。曲全体の作り直し、インターネットへのアクセス、設定やライブラリーの変更は不要です。操作手順の説明だけで終わらず、保存して確認できる結果を残してください。"
    }
    $instruction = if ($Case -eq 'editing') {
        'Rename the lead track to TrialLead. Set TrialLead gain to -7.5 dB and pan to -0.25. Add a bus track named TrialBus. On TrialLead create a new empty clip named Manual at bar 5 lasting 2 bars. In that new clip add exactly C4, E4, G4 at bar 5 beats 1, 2, 3 respectively, each lasting 0.5 beats at velocity 0.8. Preserve the original generated clips and bass. Read back the result and finish.'
    } elseif ($Case -eq 'production') {
        'On the lead track add one compressor effect, connect the bass track to its sidechain input, and set the compressor threshold to -24 dB. Add a linear lead gain automation lane from -12 dB at absolute quarter-note beat 0 to -3 dB at beat 8. Preserve all existing notes. Discover the real effect and parameter keys, read back the result, and finish.'
    } elseif ($Case -eq 'critique') {
        'Use listen to audition bar 1 for one bar and obtain a neutral critique of instrument balance. Based on that critique, adjust exactly one track fader to improve the balance. Choose the track and amount from the critique and explain your reason. Read back the saved value, then use listen again on the same range with the first audio_path as compare_to. Compare the actual critiques. Preserve every note and every other setting. If the critic cannot determine the sounds or does not support an adjustment, explain the limitation and abstain from editing. Never invent audible observations or claim an unverified improvement.'
    } else {
        'Use listen to audition bar 1 for one bar and get an audio critique. Read the lead fader, lower it by exactly 6 dB, save, then use listen again on exactly the same one-bar range. On the second call pass the first result audio_path as compare_to, so both recordings are provided for comparison. Compare the two actual critiques and report what changed. Preserve every note. If the critic cannot determine the sounds or cannot hear them, say so honestly; still perform the requested gain change and second listening call. Never invent audible observations or claim a failed critique was successful.'
    }
    return "Edit this existing project in place: $Project`n$instruction`nUse the available tools now. All writes must stay in this project directory. Do not compose a replacement song, export a full mix, access the internet, or change saved preferences or libraries. A saved and independently inspectable result is required; do not merely describe intended actions."
}

function Invoke-Control($Wire, $Project, $Case) {
    if ($Case -in @('listening','critique')) { throw 'Listening requires a model trial; use editing or production for deterministic controls' }
    if ($Case -eq 'editing') {
        Assert-Tool (Invoke-Tool $Wire 'rename_track' @{project=$Project; track='lead'; name='TrialLead'})
        Assert-Tool (Invoke-Tool $Wire 'set_level' @{project=$Project; track='TrialLead'; gain_db=-7.5; pan=-0.25})
        Assert-Tool (Invoke-Tool $Wire 'add_track' @{project=$Project; name='TrialBus'; kind='bus'})
        Assert-Tool (Invoke-Tool $Wire 'add_clip' @{project=$Project; track='TrialLead'; name='Manual'; start_bar=5; bars=2})
        $saved = Get-Content -LiteralPath $Project -Raw | ConvertFrom-Json -AsHashtable -Depth 100
        $track = @($saved.tracks | Where-Object name -EQ TrialLead)[0]
        $clipNumber = $track.kind.clips.Count
        $notes = @(@{pitch='C4'; bar=5; beat=1; beats=0.5; velocity=0.8}, @{pitch='E4'; bar=5; beat=2; beats=0.5; velocity=0.8}, @{pitch='G4'; bar=5; beat=3; beats=0.5; velocity=0.8})
        Assert-Tool (Invoke-Tool $Wire 'edit_notes' @{project=$Project; track='TrialLead'; clip=$clipNumber; add=$notes})
    } else {
        Assert-Tool (Invoke-Tool $Wire 'effects' @{project=$Project; track='lead'; operation=@{action='add'; effect='auris.fx.compressor'}})
        $saved = Get-Content -LiteralPath $Project -Raw | ConvertFrom-Json -AsHashtable -Depth 100
        $track = @($saved.tracks | Where-Object name -EQ lead)[0]
        $slot = $track.mixer.effects.Count
        Assert-Tool (Invoke-Tool $Wire 'effects' @{project=$Project; track='lead'; operation=@{action='sidechain'; slot=$slot; source='bass'}})
        Assert-Tool (Invoke-Tool $Wire 'set_effect' @{project=$Project; track='lead'; slot=$slot; param='threshold_db'; value=-24})
        $parameters = Invoke-Tool $Wire 'automation' @{project=$Project; track='lead'; target=@{kind='mixer'}; operation=@{action='read'}}
        Assert-Tool $parameters
        $listed = $parameters.content[0].text | ConvertFrom-Json -AsHashtable
        $gain = @($listed.parameters | Where-Object { $_.name -match 'gain|volume' -or $_.key -match 'gain|volume' })
        if ($gain.Count -ne 1) { throw 'Expected one mixer gain parameter' }
        Assert-Tool (Invoke-Tool $Wire 'automation' @{project=$Project; track='lead'; target=@{kind='mixer'}; operation=@{action='set'; param=$gain[0].key; points=@(@{beat=0; value=-12}, @{beat=8; value=-3}); replace=$true; curve='linear'}})
    }
}

function Invoke-McpModel($Wire, $Info, $Definitions, $Project, $Prompt, $Model, $Directory) {
    $messages = [Collections.Generic.List[object]]::new()
    $messages.Add(@{role='system'; content=$Info.instructions})
    $messages.Add(@{role='user'; content=$Prompt})
    $tools = @($Definitions.tools | ForEach-Object { @{type='function'; function=@{name=$_.name; description=$_.description; parameters=$_.inputSchema}} })
    $log = Join-Path $Directory 'ollama.jsonl'
    $callCount = 0
    $errorCount = 0
    $heard = [Collections.Generic.List[string]]::new()
    for ($turn=1; $turn -le $MaxTurns; $turn++) {
        $request = @{model=$Model; messages=$messages.ToArray(); tools=$tools; stream=$false; think=$false; options=@{num_ctx=$ContextTokens; temperature=0; seed=71421; num_predict=4096}}
        Add-Event $log @{direction='request'; payload=$request}
        $reply = Invoke-RestMethod -Uri ($OllamaUrl.TrimEnd('/') + '/api/chat') -Method Post -ContentType 'application/json' -Body (ConvertTo-Json -InputObject $request -Depth 100 -Compress) -TimeoutSec $TimeoutSeconds
        $reply = $reply | ConvertTo-Json -Depth 100 | ConvertFrom-Json -AsHashtable -Depth 100
        Add-Event $log @{direction='response'; payload=$reply}
        if ($reply.done_reason -eq 'length') {
            return @{completed=$false; calls=$callCount; errors=$errorCount; turns=$turn; error='Model output reached the 4096-token limit; this response was not executed or accepted as completion.'; listening_results=$heard.ToArray()}
        }
        $messages.Add($reply.message)
        if (!$reply.message.tool_calls) { return @{completed=$true; calls=$callCount; errors=$errorCount; turns=$turn; answer=$reply.message.content; listening_results=$heard.ToArray()} }
        foreach ($call in $reply.message.tool_calls) {
            $callCount++
            $name = $call.function.name
            $arguments = $call.function.arguments
            # Keep model-selected mutation destinations in this disposable fixture.
            $allowed = @('capabilities','tool_help','describe','inspect_composition','analyze_music','mixer','notes','listen','effects','automation','routing','set_effect','set_level','add_track','add_clip','edit_notes','rename_track','list_instruments','list_presets','list_progressions','spec_reference','search_documentation')
            $safeProject = !$arguments.project -or ([IO.Path]::GetFullPath($arguments.project, $Directory) -eq $Project)
            if ($name -notin $allowed -or !$safeProject) {
                $result = @{isError=$true; content=@(@{type='text'; text='This smoke trial only permits inspecting or editing its existing project; use its exact path and the tools required by the task.'})}
            } else { $result = Invoke-Tool $Wire $name $arguments }
            if ($result.isError) { $errorCount++ }
            $text = @($result.content | Where-Object type -EQ text | ForEach-Object text) -join "`n"
            if ($name -eq 'listen' -and !$result.isError) { $heard.Add($text) }
            $messages.Add(@{role='tool'; tool_name=$name; tool_call_id=$call.id; content=$text})
        }
    }
    return @{completed=$false; calls=$callCount; errors=$errorCount; turns=$MaxTurns; error='Model turn budget exhausted'; listening_results=$heard.ToArray()}
}

function Invoke-RigModel($Project, $Prompt, $Model, $Directory) {
    $wire = Start-Wire $agentBinary $Directory @('--json','--provider','ollama','--url',$OllamaUrl,'--model',$Model,'--context-tokens',"$ContextTokens",'--thinking','off','--max-turns',"$MaxTurns")
    $calls=0; $errors=0
    $heard = [Collections.Generic.List[string]]::new()
    try {
        do { $event = Read-Wire $wire; if ($event.event -eq 'error') { throw $event.message } } while ($event.event -ne 'ready')
        Send-Wire $wire @{say=$Prompt}
        while ($true) {
            $event = Read-Wire $wire
            if ($event.event -eq 'call') { $calls++ }
            if ($event.event -eq 'result' -and !$event.ok) { $errors++ }
            if ($event.event -eq 'result' -and $event.ok -and $event.tool -eq 'listen') { $heard.Add($event.text) }
            if ($event.event -eq 'answer') { return @{completed=$true; calls=$calls; errors=$errors; answer=$event; listening_results=$heard.ToArray()} }
            if ($event.event -eq 'error') { return @{completed=$false; calls=$calls; errors=$errors; error=$event.message; listening_results=$heard.ToArray()} }
        }
    } finally { Stop-Wire $wire }
}

function Get-ThresholdResult($Saved) {
    $lead = @($Saved.tracks | Where-Object name -EQ lead)
    if ($lead.Count -ne 1) { return @{passed=$false; realization='missing_lead'} }
    $compressors = @($lead[0].mixer.effects | Where-Object effect_id -EQ 'auris.fx.compressor')
    if ($compressors.Count -ne 1) { return @{passed=$false; realization='ambiguous_compressor'} }
    $effect = $compressors[0]
    $lanes = @($Saved.automation | Where-Object {
        $_.target.Effect.track -eq $lead[0].id -and $_.target.Effect.slot -eq $effect.id -and $_.key -eq 'threshold_db'
    })
    if ($lanes.Count) {
        # AutomationLane::value_at holds the nearest endpoint outside the written range.
        # A constant lane therefore sets the threshold across the whole render interval.
        $constant = $lanes.Count -eq 1 -and $lanes[0].points.Count -gt 0 -and @($lanes[0].points | Where-Object { [math]::Abs($_.value + 24) -ge 0.001 }).Count -eq 0
        return @{passed=$constant; realization='automation'; lanes=$lanes}
    }
    $value = $effect.state.params.threshold_db
    return @{passed=($null -ne $value -and [math]::Abs($value + 24) -lt 0.001); realization='static'; value=$value}
}

function Get-Verdict($Project, $Before, $Case) {
    $saved = Get-Content -LiteralPath $Project -Raw | ConvertFrom-Json -AsHashtable -Depth 100
    $checks = [ordered]@{}
    $bass = @($saved.tracks | Where-Object name -EQ bass)
    $oldBass = @($Before.tracks | Where-Object name -EQ bass)[0]
    $checks.bass_preserved = $bass.Count -eq 1 -and (ConvertTo-Json $bass[0].kind -Depth 100 -Compress) -eq (ConvertTo-Json $oldBass.kind -Depth 100 -Compress)
    $leadName = if ($Case -eq 'editing') { 'TrialLead' } else { 'lead' }
    $lead = @($saved.tracks | Where-Object name -EQ $leadName)
    $checks.lead_exists = $lead.Count -eq 1
    if ($lead.Count -ne 1) { return $checks }
    $lead = $lead[0]
    $oldLead = @($Before.tracks | Where-Object name -EQ lead)[0]
    $original = @($lead.kind.clips | Where-Object { $_.id -in $oldLead.kind.clips.id })
    $checks.generated_clips_preserved = (ConvertTo-Json -InputObject $original -Depth 100 -Compress) -eq (ConvertTo-Json -InputObject @($oldLead.kind.clips) -Depth 100 -Compress)
    if ($Case -eq 'editing') {
        $bus = @($saved.tracks | Where-Object name -EQ TrialBus)
        $checks.real_bus = $bus.Count -eq 1 -and $bus[0].kind.type -eq 'bus'
        $checks.gain = [math]::Abs($lead.mixer.gain_db + 7.5) -lt 0.001
        $checks.pan = [math]::Abs($lead.mixer.pan + 0.25) -lt 0.001
        $manual = @($lead.kind.clips | Where-Object name -EQ Manual)
        $checks.manual_clip = $manual.Count -eq 1
        if ($manual.Count -eq 1) {
            $clip = $manual[0]
            # Auris stores 960 ticks per quarter note. Bar 5 in this 4/4 fixture is beat 16.
            $checks.clip_range = $clip.start -eq 15360 -and $clip.length -eq 7680
            $placed = @($clip.notes | Sort-Object start)
            $checks.exact_notes = $placed.Count -eq 3 -and ($placed.pitch -join ',') -eq '60,64,67' -and ($placed.start -join ',') -eq '0,960,1920' -and @($placed | Where-Object { $_.length -ne 480 -or [math]::Abs($_.velocity - 0.8) -gt 0.001 }).Count -eq 0
        }
    } elseif ($Case -eq 'production') {
        $compressors = @($lead.mixer.effects | Where-Object effect_id -EQ 'auris.fx.compressor')
        $checks.one_compressor = $compressors.Count -eq 1
        if ($compressors.Count -eq 1) {
            $checks.sidechain = $compressors[0].sidechain -eq $bass[0].id
            $checks.threshold = (Get-ThresholdResult $saved).passed
        }
        $lanes = @($saved.automation | Where-Object { $_.target.TrackGain -eq $lead.id })
        $checks.gain_lane = $lanes.Count -eq 1
        if ($lanes.Count -eq 1) {
            $checks.gain_curve = $lanes[0].curve -eq 'Linear' -and ($lanes[0].points.value -join ',') -eq '-12,-3' -and ($lanes[0].points.tick -join ',') -eq '0,7680'
        }
    } elseif ($Case -eq 'listening') {
        $checks.gain_reduced_six_db = [math]::Abs($lead.mixer.gain_db - ($oldLead.mixer.gain_db - 6)) -lt 0.001
    } else {
        $changed = 0
        foreach ($track in $saved.tracks) {
            $old = @($Before.tracks | Where-Object id -EQ $track.id)
            if ($old.Count -eq 1) {
                if ([math]::Abs($track.mixer.gain_db - $old[0].mixer.gain_db) -gt 0.001) { $changed++ }
                $track.mixer.gain_db = $old[0].mixer.gain_db
            }
        }
        $checks.one_fader_changed = $changed -eq 1
        $checks.other_track_state_preserved = (ConvertTo-Json -InputObject $saved.tracks -Depth 100 -Compress) -eq (ConvertTo-Json -InputObject $Before.tracks -Depth 100 -Compress)
    }
    return $checks
}

$summary = [Collections.Generic.List[object]]::new()
Write-Json (Join-Path $sessionDirectory 'environment.json') @{context_tokens=$ContextTokens; thinking=$false; max_turns=$MaxTurns; timeout_seconds=$TimeoutSeconds; prompt_language=$PromptLanguage; mcp_sampling=@{temperature=0; seed=71421; num_predict=4096}; rig_sampling='Application request defaults; identify the build by its binary hash'; ollama=$OllamaUrl; binaries=@(Get-FileHash -LiteralPath $mcpBinary,$agentBinary -Algorithm SHA256 | Select-Object Path,Hash)}
$trialModels = if ($Transport.Count -eq 1 -and $Transport[0] -eq 'control') { @('control') } else { $Models }
foreach ($model in $trialModels) {
    if ($model -ne 'control') {
        $shown = Invoke-RestMethod -Uri ($OllamaUrl.TrimEnd('/') + '/api/show') -Method Post -ContentType 'application/json' -Body (@{model=$model} | ConvertTo-Json) -TimeoutSec 60
        Write-Json (Join-Path $sessionDirectory (($model -replace '[^a-zA-Z0-9.-]', '_') + '.model.json')) $shown
    }
    foreach ($door in $Transport) {
        foreach ($case in $Scenario) {
            $directory = Join-Path $sessionDirectory (($model -replace '[^a-zA-Z0-9.-]', '_') + '-' + $door + '-' + $case)
            $null = New-Item -ItemType Directory -Path $directory
            Write-Host "START $model $door $case"
            $watch = [Diagnostics.Stopwatch]::StartNew()
            $wire = $null
            $result = @{model=$model; transport=$door; scenario=$case; directory=$directory; passed=$false}
            try {
                $wire = Start-Wire $mcpBinary $directory
                $info = Initialize-Mcp $wire
                $definitions = Invoke-Mcp $wire 'tools/list' @{}
                Write-Json (Join-Path $directory 'tools.json') $definitions
                $project = New-Fixture $wire $directory $case
                $before = Get-Content -LiteralPath $project -Raw | ConvertFrom-Json -AsHashtable -Depth 100
                Write-Json (Join-Path $directory 'before.json') $before
                $prompt = Get-Prompt $project $case
                [IO.File]::WriteAllText((Join-Path $directory 'prompt.txt'), $prompt, $utf8)
                if ($door -eq 'control') { Invoke-Control $wire $project $case; $run=@{completed=$true; calls=0; errors=0} }
                elseif ($door -eq 'mcp') { $run = Invoke-McpModel $wire $info $definitions $project $prompt $model $directory }
                else { $run = Invoke-RigModel $project $prompt $model $directory }
                $result.run = $run
                $result.checks = Get-Verdict $project $before $case
                if ($case -in @('listening','critique')) {
                    $result.checks.two_listening_results = $run.listening_results.Count -ge 2
                    $result.assessment_scope = 'Checks cover repeated tool execution and persisted edits. Inspect listening_results independently for actual audible accuracy; an HTTP success or a fluent critique does not establish hearing.'
                    if ($run.listening_results.Count -ge 2) {
                        $first = $run.listening_results[0] | ConvertFrom-Json -AsHashtable -Depth 30
                        $second = $run.listening_results[-1] | ConvertFrom-Json -AsHashtable -Depth 30
                        $result.checks.audio_submitted_both_times = $first.audio_sent -eq $true -and $second.audio_sent -eq $true
                        $result.checks.distinct_recordings = $first.audio_path -ne $second.audio_path
                        $result.checks.comparison_names_first_recording = $second.compare_to -and [IO.Path]::GetFullPath($second.compare_to).Replace('\\?\','') -eq [IO.Path]::GetFullPath($first.audio_path).Replace('\\?\','')
                        $hashes = @(Get-FileHash -LiteralPath $first.audio_path,$second.audio_path -Algorithm SHA256 | Select-Object Path,Hash)
                        $result.recordings = $hashes
                        $result.checks.audio_bytes_changed = $hashes.Count -eq 2 -and $hashes[0].Hash -ne $hashes[1].Hash
                    }
                }
                $result.passed = $run.completed -and !($result.checks.Values -contains $false)
                if ($case -eq 'production') {
                    $result.threshold = Get-ThresholdResult (Get-Content -LiteralPath $project -Raw | ConvertFrom-Json -AsHashtable -Depth 100)
                }
                if ($case -eq 'critique') {
                    $saved = Get-Content -LiteralPath $project -Raw | ConvertFrom-Json -AsHashtable -Depth 100
                    $result.fader_changes = @($saved.tracks | ForEach-Object {
                        $track = $_
                        $old = @($before.tracks | Where-Object id -EQ $track.id)
                        if ($old.Count -eq 1 -and [math]::Abs($track.mixer.gain_db - $old[0].mixer.gain_db) -gt 0.001) {
                            @{track=$track.name; id=$track.id; before_db=$old[0].mixer.gain_db; after_db=$track.mixer.gain_db}
                        }
                    })
                }
                Assert-Tool (Invoke-Tool $wire 'describe' @{project=$project})
                Assert-Tool (Invoke-Tool $wire 'mixer' @{project=$project})
                Write-Json (Join-Path $directory 'after.json') (Get-Content -LiteralPath $project -Raw | ConvertFrom-Json -AsHashtable -Depth 100)
            } catch { $result.error=$_.ToString() }
            finally { Stop-Wire $wire }
            $result.elapsed_seconds = [math]::Round($watch.Elapsed.TotalSeconds, 3)
            Write-Json (Join-Path $directory 'result.json') $result
            $summary.Add($result)
            Write-Json (Join-Path $sessionDirectory 'summary.json') $summary.ToArray()
            Write-Host "END $model $door $case passed=$($result.passed) elapsed=$($result.elapsed_seconds)s $($result.error)"
        }
    }
}
Write-Host "Results: $sessionDirectory"
if (@($summary | Where-Object { !$_.passed }).Count) { exit 1 }
