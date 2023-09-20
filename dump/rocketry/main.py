from rocketry import Rocketry
from rocketry.conds import after_success, after_fail, after_finish
from rocketry.log import MinimalRecord
from redbird.repos import CSVFileRepo
import subprocess

repo = CSVFileRepo(filename="tasks.csv", model=MinimalRecord)

app = Rocketry(logger_repo=repo)

@app.task('daily')
def job():
    print ("Running a cronjob...")
    subprocess.call(['sh', '/app/rocketry/dump.sh'])
    return(True)

@app.task(after_success(job))
def do_after_success():
    print ("Trying to backup...")
    subprocess.call(['sh', '/app/rocketry/backup.sh'])

@app.task(after_fail(job))
def do_after_fail():
    print(False)

@app.task(after_finish(job))
def do_after_fail_or_success():
    print ("Backup done")

app.run()