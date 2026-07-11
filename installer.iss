; Script de Inno Setup para compilar el instalador visual de Windows para ITO
; Descargar Inno Setup gratis desde: https://jrsoftware.org/isinfo.php

[Setup]
AppName=ITO
AppVersion=0.3.2
AppPublisher=ITO Gravity Team
AppPublisherURL=https://github.com/ITO-Gravity
DefaultDirName={autopf}\ITO
DefaultGroupName=ITO
OutputBaseFilename=ito-installer-x86_64
Compression=lzma
SolidCompression=yes
WizardStyle=modern
; Solicitar privilegios de administrador para escribir en Program Files y PATH del sistema
PrivilegesRequired=admin

[Files]
; Copiar el ejecutable principal y sus envoltorios compilados en modo release
Source: "target\release\_ito.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "target\release\ito.cmd"; DestDir: "{app}"; Flags: ignoreversion
Source: "LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\ITO Command Line"; Filename: "cmd.exe"; Parameters: "/k cd /d %USERPROFILE%"

[Run]
; Ejecutar de forma silenciosa el inicializador de envoltorios de consola tras terminar la instalación
Filename: "{app}\_ito.exe"; StatusMsg: "Configurando integraciones del sistema..."; Flags: runhidden

[Registry]
; Agregar de forma segura el directorio de instalación al PATH del sistema de Windows (HKLM)
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Check: NeedsAddPath(ExpandConstant('{app}'))

[Code]
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', OrigPath) then
  begin
    Result := Pos(';' + UpperCase(Param) + ';', ';' + UpperCase(OrigPath) + ';') = 0;
  end
  else
  begin
    Result := True;
  end;
end;

procedure CurUninstallStepChanged(JustAfterAnUninstallStep: TUninstallStep);
var
  OrigPath, PathToRemove, NewPath: string;
  PathIndex: Integer;
begin
  if JustAfterAnUninstallStep = usPostUninstall then
  begin
    PathToRemove := ExpandConstant('{app}');
    if RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', OrigPath) then
    begin
      // Limpiar de forma segura la ruta al desinstalar
      PathIndex := Pos(';' + UpperCase(PathToRemove), ';' + UpperCase(OrigPath));
      if PathIndex > 0 then
      begin
        NewPath := OrigPath;
        Delete(NewPath, PathIndex, Length(PathToRemove) + 1);
        RegWriteExpandStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', NewPath);
      end;
    end;
  end;
end;
