"""Build private sic-core v0.1 artifacts from an allowlisted snapshot and test clean installs.

The checkout is read only. Builds, downloaded build dependencies, logs and reports
are written only to an explicitly selected new output directory or owned temp dirs.
No service, provider, package index publication or repository publication is invoked.
"""
import argparse
from email.parser import BytesParser
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import venv
import zipfile

ROOT=Path(__file__).resolve().parents[1]
DATA=('operations.json','task_ledger.schema.json','config.example.json','requirements.txt')
SUPPORT=('pyproject.toml','MANIFEST.in','LICENSE','docs/python-core-package.md','scripts/build_python_core.py','scripts/verify_installed_core.py')


def sha(data):return hashlib.sha256(data).hexdigest()

def sources():
    paths=[*sorted((ROOT/'sic_core').glob('*.py')),*[ROOT/'sic_core'/name for name in DATA],*[ROOT/name for name in SUPPORT]]
    if any(path.is_symlink() or not path.resolve().is_relative_to(ROOT) for path in paths):raise ValueError('Release source must be regular checkout files, not external links')
    return {path.relative_to(ROOT).as_posix():path.read_bytes() for path in paths}

def hashes(files):return {name:sha(data) for name,data in sorted(files.items())}

def identity(files):return sha(json.dumps(hashes(files),sort_keys=True,separators=(',',':')).encode())

def run(command,cwd,env,log):
    result=subprocess.run([str(item) for item in command],cwd=cwd,env=env,text=True,encoding='utf-8',errors='replace',capture_output=True)
    log.write_text(result.stdout+'\n'+result.stderr,encoding='utf-8')
    if result.returncode:raise RuntimeError('Subprocess failed; inspect '+log.name)
    return result.stdout

def validate_config(files):
    config=tomllib.loads(files['pyproject.toml'].decode('utf-8-sig'))
    assert config['project']['name']=='sic-core' and config['project']['version']=='0.1.0'
    assert config['project']['requires-python']=='>=3.12,<3.15' and config['project']['dependencies']==[]
    locked={line.strip() for line in files['sic_core/requirements.txt'].decode('utf-8-sig').splitlines() if line.strip() and not line.lstrip().startswith('#')}
    extras=config['project']['optional-dependencies']
    assert set(extras['full'])==locked and set(extras['provider'])|set(extras['media'])==locked
    assert config['tool']['setuptools']['packages']==['sic_core']
    return config

def inspect_archive(path,files):
    if path.suffix=='.whl':
        with zipfile.ZipFile(path) as archive:
            if any(stat.S_IFMT(info.external_attr>>16)==stat.S_IFLNK for info in archive.infolist()):raise ValueError('Linked archive members are not release files')
            entries={name:archive.read(name) for name in archive.namelist() if not name.endswith('/')}
        def allowed(name):
            return name in files and name.startswith('sic_core/') or name=='sic_core-0.1.0.dist-info/licenses/LICENSE' or re.fullmatch(r'sic_core-0\.1\.0\.dist-info/(METADATA|WHEEL|RECORD|entry_points\.txt|top_level\.txt)',name) is not None
    else:
        with tarfile.open(path,'r:gz') as archive:
            all_members=archive.getmembers()
            if any(not (member.isfile() or member.isdir()) for member in all_members):raise ValueError('Linked or special source archive members are not release files')
            allowed_dirs={'sic_core-0.1.0','sic_core-0.1.0/sic_core','sic_core-0.1.0/docs','sic_core-0.1.0/scripts','sic_core-0.1.0/sic_core.egg-info'}
            if any(member.name.rstrip('/') not in allowed_dirs for member in all_members if member.isdir()):raise ValueError('Source archive directory outside explicit release allowlist')
            members=[member for member in all_members if member.isfile()]
            if any(not member.name.startswith('sic_core-0.1.0/') for member in members):raise ValueError('Unexpected source distribution root')
            entries={member.name[len('sic_core-0.1.0/'):]:archive.extractfile(member).read() for member in members}
        def allowed(name):return name in files or name in ('PKG-INFO','setup.cfg') or re.fullmatch(r'sic_core\.egg-info/(PKG-INFO|SOURCES\.txt|dependency_links\.txt|entry_points\.txt|requires\.txt|top_level\.txt)',name) is not None
    unexpected=[name for name in entries if not allowed(name)]
    if unexpected:raise ValueError('Archive contains files outside the explicit release allowlist: '+', '.join(unexpected))
    core_files={name:data for name,data in files.items() if name.startswith('sic_core/')}
    expected_files=core_files if path.suffix=='.whl' else files
    if any(name not in entries or entries[name]!=data for name,data in expected_files.items()):raise ValueError('Archived source differs from its explicit source snapshot')
    metadata_name='sic_core-0.1.0.dist-info/METADATA' if path.suffix=='.whl' else 'PKG-INFO'
    metadata=BytesParser().parsebytes(entries[metadata_name])
    if metadata['Name']!='sic-core' or metadata['Version']!='0.1.0':raise ValueError('Distribution metadata identity changed')
    if metadata.get('License-Expression')!='MIT':raise ValueError('Distribution license must be author-approved MIT')
    license_name='sic_core-0.1.0.dist-info/licenses/LICENSE' if path.suffix=='.whl' else 'LICENSE'
    if entries.get(license_name)!=files['LICENSE']:raise ValueError('Author-approved license missing from archive')
    if any('; extra ==' not in requirement for requirement in metadata.get_all('Requires-Dist',[])):raise ValueError('Unexpected mandatory base dependency')
    private_roots=[str(ROOT.resolve()),str(Path.home().resolve())]
    forbidden=[]
    for value in private_roots:
        forbidden.extend([value.encode(),value.replace('\\','/').encode(),value.replace('/','\\').encode(),json.dumps(value)[1:-1].encode()])
    secrets=[value.encode() for key,value in os.environ.items() if value and len(value)>=16 and key.upper().endswith(('_KEY','_TOKEN','_SECRET','_PASSWORD'))]
    for name,data in entries.items():
        if any(token and token.lower() in data.lower() for token in forbidden):raise ValueError('Private host path detected in archive member '+name)
        if any(secret in data for secret in secrets):raise ValueError('Host credential detected in archive; value withheld')
        if re.search(rb'(?:sk-(?:proj-)?)[A-Za-z0-9_-]{24,}',data):raise ValueError('Credential-shaped literal detected in archive; value withheld')
    return {'files':sorted(entries),'file_count':len(entries),'core_file_sha256':hashes(core_files),'sha256':sha(path.read_bytes()),'bytes':path.stat().st_size,'private_host_paths':False,'host_credentials':False,'non_allowlisted_files':False}

def audit_probes(wheel,files):
    with zipfile.ZipFile(wheel) as archive:original={name:archive.read(name) for name in archive.namelist() if not name.endswith('/')}
    with tempfile.TemporaryDirectory(prefix='sic-core-audit-probes-') as temporary:
        directory=Path(temporary).resolve();assert directory.parent==Path(tempfile.gettempdir()).resolve() and directory.name.startswith('sic-core-audit-probes-')
        metadata='sic_core-0.1.0.dist-info/METADATA';credential=os.urandom(24).hex();key='SIC_DISTRIBUTION_AUDIT_TOKEN';old=os.environ.get(key)
        cases=[('unapproved_paper_member',{'paper/unapproved.md':b'Owned synthetic rejection fixture'},'outside the explicit release allowlist'),
               ('private_host_path',{metadata:original[metadata]+b'\n'+str(Path.home()).encode()},'Private host path'),
               ('changed_source',{'sic_core/__init__.py':original['sic_core/__init__.py']+b'\n# changed fixture\n'},'Archived source differs'),
               ('host_credential',{metadata:original[metadata]+b'\n'+credential.encode()},'Host credential')]
        passed=[]
        try:
            for label,changes,expected in cases:
                if label=='host_credential':os.environ[key]=credential
                path=directory/(label+'.whl')
                with zipfile.ZipFile(path,'w') as archive:
                    for name,data in (original|changes).items():archive.writestr(name,data)
                try:inspect_archive(path,files)
                except ValueError as error:
                    if expected not in str(error):raise
                    passed.append(label)
                else:raise AssertionError('Invalid release artifact was accepted: '+label)
        finally:
            if old is None:os.environ.pop(key,None)
            else:os.environ[key]=old
        return passed


def clean_install(artifact,output,env,build_links):
    with tempfile.TemporaryDirectory(prefix='sic-core-installed-') as temporary:
        base=Path(temporary).resolve()
        assert base.parent==Path(tempfile.gettempdir()).resolve() and base.name.startswith('sic-core-installed-')
        environment=base/'venv';work=base/'work';work.mkdir();venv.EnvBuilder(with_pip=True,system_site_packages=False).create(environment)
        python=environment/('Scripts/python.exe' if os.name=='nt' else 'bin/python')
        label='wheel' if artifact.suffix=='.whl' else 'sdist'
        command=[python,'-I','-m','pip','install','--no-input','--no-index','--no-deps','--disable-pip-version-check']
        if build_links:command.extend(['--find-links',build_links])
        command.append(artifact)
        run(command,work,env,output/(label+'-install.log'))
        copied=work/'verify_installed_core.py';copied.write_bytes((ROOT/'scripts/verify_installed_core.py').read_bytes())
        stdout=run([python,'-I',copied],work,env,output/(label+'-selfcheck.log'))
        result=json.loads(stdout.strip().splitlines()[-1]);assert result['status']=='passed'
        cli=environment/('Scripts/sic-core.exe' if os.name=='nt' else 'bin/sic-core')
        help_text=run([cli,'--help'],work,env,output/(label+'-cli-help.log'));assert '--config' in help_text
        result['cli_help_passed_without_starting_service']=True;result['original_checkout_PYTHONPATH_used']=False;result['working_directory_outside_checkout']=not work.is_relative_to(ROOT)
        return result

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path);parser.add_argument('--phase',choices=['provisional','final'],default='provisional');parser.add_argument('--expected-source-sha256');parser.add_argument('--inspect-source',action='store_true');args=parser.parse_args()
    files=sources();config=validate_config(files);source_id=identity(files)
    if args.inspect_source:print(json.dumps({'source_sha256':source_id,'files':hashes(files)},indent=2));return
    if args.output is None:parser.error('--output is required for a build')
    if args.phase=='final' and not args.expected_source_sha256:parser.error('Final artifacts require an explicit frozen --expected-source-sha256')
    if args.expected_source_sha256 and args.expected_source_sha256!=source_id:raise ValueError('Frozen source identity does not match current release inputs')
    output=args.output.resolve()
    if output.exists() and any(output.iterdir()):raise ValueError('Choose a new empty output directory; prior artifacts are never overwritten')
    output.mkdir(parents=True,exist_ok=True)
    env={key:value for key,value in os.environ.items() if key not in ('PYTHONPATH','PYTHONHOME','PIP_TARGET','PIP_PREFIX')};env['PYTHONIOENCODING']='utf-8';env['PIP_DISABLE_PIP_VERSION_CHECK']='1'
    report={'phase':args.phase,'status':'building','distribution':'sic-core','version':'0.1.0','source_sha256':source_id,'source_files':hashes(files),'source_frozen':args.phase=='final','services_started':False,'external_provider_calls':0,'publication_performed':False}
    try:
        with tempfile.TemporaryDirectory(prefix='sic-core-build-') as temporary:
            stage=Path(temporary).resolve();assert stage.parent==Path(tempfile.gettempdir()).resolve() and stage.name.startswith('sic-core-build-')
            for name,data in files.items():target=stage/name;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(data)
            program='from setuptools import build_meta; build_meta.build_sdist('+repr(str(output))+'); build_meta.build_wheel('+repr(str(output))+')'
            run([sys.executable,'-c',program],stage,env,output/'build.log')
        artifacts=[*output.glob('*.whl'),*output.glob('*.tar.gz')];assert len(artifacts)==2
        report['artifacts']={path.name:inspect_archive(path,files) for path in artifacts}
        report['rejected_negative_probes']=audit_probes(next(path for path in artifacts if path.suffix=='.whl'),files)
        build_links=output/'build-dependencies';build_links.mkdir()
        run([sys.executable,'-m','pip','download','--only-binary=:all:','--no-deps','--disable-pip-version-check','--dest',build_links,*config['build-system']['requires']],output,env,output/'build-dependency-download.log')
        report['clean_installs']={('wheel' if path.suffix=='.whl' else 'sdist'):clean_install(path,output,env,build_links) for path in artifacts}
        if identity(sources())!=source_id:raise ValueError('Release inputs changed during the build; this snapshot cannot be marked current')
        report['status']='passed';report['source_unchanged_during_build']=True
    except Exception as error:
        report['status']='failed';report['error_type']=type(error).__name__;report['error']=str(error);raise
    finally:(output/'verification.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps({'phase':args.phase,'status':'passed','source_sha256':source_id,'artifacts':{name:{'sha256':data['sha256'],'bytes':data['bytes']} for name,data in report['artifacts'].items()},'clean_installs':['wheel','sdist']},ensure_ascii=False))

if __name__=='__main__':main()
