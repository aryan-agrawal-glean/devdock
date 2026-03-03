load('ext://uibutton', 'choice_input', 'cmd_button', 'location')
load('ext://uibutton', 'cmd_button', 'location', 'text_input')

def toggle_hot_reload():
    cmd_button('toggle_hot_reload',
        argv=['sh', '-c', os.getenv("DEVDOCK_ROOT") + "/devdock/core/lib/toggle_hot_reload.sh"],
        text='Toggle hot reload',
        location=location.NAV,
        icon_name='toggle_off',
        inputs = [choice_input(name = "reload", choices = ["enable", "disable"])])

    cmd_button('report_error',
        argv=['sh', '-c', os.getenv("DEVDOCK_ROOT") + "/devdock/core/lib/report_error.sh"],
        text='Report error',
        location=location.NAV,
        icon_name='support_agent',
        inputs = [
            text_input(
                'brief_summary',
                label='Summary',
                default='',
                placeholder='Brief summary of the issue'
            ),
            text_input(
                'detailed_description',
                label='Description',
                default='',
                placeholder='Detailed description of the issue'
            ),
            text_input(
                'command',
                label='Command',
                default='devdock start',
                placeholder='Command that triggered the issue'
            )
        ])
