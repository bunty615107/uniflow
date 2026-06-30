import sys
from kivy.app import App
from kivy.uix.boxlayout import BoxLayout
from kivy.uix.label import Label
from kivy.uix.textinput import TextInput
from kivy.uix.button import Button
from kivy.uix.scrollview import ScrollView
from kivy.uix.gridlayout import GridLayout
from kivy.network.urlrequest import UrlRequest
from kivy.clock import Clock
import json

class UniFlowClient(BoxLayout):
    def __init__(self, **kwargs):
        super(UniFlowClient, self).__init__(**kwargs)
        self.orientation = 'vertical'
        self.padding = 20
        self.spacing = 10

        # --- Title ---
        self.add_widget(Label(
            text='UniFlow Android Client',
            font_size='24sp',
            size_hint_y=None,
            height=40,
            bold=True,
            color=(0, 0.86, 0.86, 1)  # Cyan
        ))

        # --- Connection Config ---
        config_layout = BoxLayout(orientation='horizontal', size_hint_y=None, height=40, spacing=10)
        config_layout.add_widget(Label(text='Daemon IP:', size_hint_x=0.2))
        self.ip_input = TextInput(text='192.168.1.100:7878', multiline=False, size_hint_x=0.5)
        config_layout.add_widget(self.ip_input)
        
        config_layout.add_widget(Label(text='API Key:', size_hint_x=0.15))
        self.key_input = TextInput(text='dev-uniflow-key-12345', password=True, multiline=False, size_hint_x=0.35)
        config_layout.add_widget(self.key_input)
        self.add_widget(config_layout)

        # --- Control Buttons ---
        btn_layout = BoxLayout(orientation='horizontal', size_hint_y=None, height=50, spacing=10)
        self.connect_btn = Button(text='Refresh Status', background_color=(0, 0.6, 0.6, 1))
        self.connect_btn.bind(on_press=self.refresh_data)
        btn_layout.add_widget(self.connect_btn)
        
        self.seed_btn = Button(text='Seed Demo Jobs', background_color=(0.5, 0.2, 0.5, 1))
        self.seed_btn.bind(on_press=self.seed_demo)
        btn_layout.add_widget(self.seed_btn)
        self.add_widget(btn_layout)

        # --- Status Header ---
        self.status_label = Label(
            text='Status: Disconnected',
            size_hint_y=None,
            height=30,
            color=(0.9, 0.3, 0.3, 1)
        )
        self.add_widget(self.status_label)

        # --- Job Submission Form ---
        form_label = Label(text='Quick Submit Job', bold=True, size_hint_y=None, height=30, color=(0.8, 0.8, 0.8, 1))
        self.add_widget(form_label)
        
        form_layout = GridLayout(cols=2, spacing=10, size_hint_y=None, height=120)
        form_layout.add_widget(Label(text='Source Path:'))
        self.src_input = TextInput(text='sample_src.bin', multiline=False)
        form_layout.add_widget(self.src_input)
        
        form_layout.add_widget(Label(text='Dest Path:'))
        self.dst_input = TextInput(text='sample_dst.bin', multiline=False)
        form_layout.add_widget(self.dst_input)
        
        form_layout.add_widget(Label(text='Zero Knowledge:'))
        self.zk_btn = Button(text='OFF', background_color=(0.3, 0.3, 0.3, 1))
        self.zk_btn.bind(on_press=self.toggle_zk)
        form_layout.add_widget(self.zk_btn)
        self.add_widget(form_layout)
        
        self.submit_btn = Button(text='Submit Job', size_hint_y=None, height=45, background_color=(0.1, 0.7, 0.3, 1))
        self.submit_btn.bind(on_press=self.submit_job)
        self.add_widget(self.submit_btn)

        # --- Jobs List (Scrollable) ---
        self.add_widget(Label(text='Active Jobs', bold=True, size_hint_y=None, height=30))
        self.scroll_view = ScrollView()
        self.jobs_layout = GridLayout(cols=1, spacing=5, size_hint_y=None)
        self.jobs_layout.bind(minimum_height=self.jobs_layout.setter('height'))
        self.scroll_view.add_widget(self.jobs_layout)
        self.add_widget(self.scroll_view)

        # Auto-refresh status every 5 seconds
        Clock.schedule_interval(self.refresh_data, 5)

    def get_headers(self):
        return {
            'Content-Type': 'application/json',
            'X-API-Key': self.key_input.text.strip()
        }

    def get_url(self, path):
        ip = self.ip_input.text.strip()
        if not ip.startswith('http://') and not ip.startswith('https://'):
            ip = 'http://' + ip
        return f"{ip}{path}"

    def refresh_data(self, *args):
        url = self.get_url('/api/status')
        UrlRequest(
            url,
            on_success=self.on_status_success,
            on_failure=self.on_connection_error,
            on_error=self.on_connection_error,
            req_headers=self.get_headers(),
            timeout=3
        )
        
        url_jobs = self.get_url('/api/jobs')
        UrlRequest(
            url_jobs,
            on_success=self.on_jobs_success,
            req_headers=self.get_headers(),
            timeout=3
        )

    def on_status_success(self, request, result):
        self.status_label.text = f"Status: Connected | Total Jobs: {result.get('jobs_total', 0)} | Running: {result.get('running', 0)}"
        self.status_label.color = (0.2, 0.8, 0.2, 1)

    def on_connection_error(self, request, error):
        self.status_label.text = "Status: Disconnected / Auth Error"
        self.status_label.color = (0.9, 0.3, 0.3, 1)

    def on_jobs_success(self, request, result):
        self.jobs_layout.clear_widgets()
        if not result:
            self.jobs_layout.add_widget(Label(text='No active jobs', size_hint_y=None, height=30))
            return
            
        for job in result:
            status = job.get('status', 'Unknown')
            progress = job.get('progress', 0.0)
            progress_str = f" ({progress:.1f}%)" if progress is not None else ""
            
            job_text = f"ID: {job.get('id')[:8]} | {job.get('label', 'Job')} | {status}{progress_str}\nSrc: {job.get('source')} -> Dst: {job.get('destination')}"
            
            color = (0.2, 0.7, 0.9, 1) if status == 'Running' else (0.7, 0.7, 0.7, 1)
            if status == 'Completed':
                color = (0.2, 0.8, 0.2, 1)
            elif status == 'Failed':
                color = (0.9, 0.3, 0.3, 1)

            lbl = Label(
                text=job_text,
                size_hint_y=None,
                height=60,
                color=color,
                halign='left',
                valign='middle'
            )
            lbl.bind(size=lbl.setter('text_size'))
            self.jobs_layout.add_widget(lbl)

    def toggle_zk(self, instance):
        if instance.text == 'OFF':
            instance.text = 'ON'
            instance.background_color = (0.1, 0.6, 0.8, 1)
        else:
            instance.text = 'OFF'
            instance.background_color = (0.3, 0.3, 0.3, 1)

    def submit_job(self, instance):
        url = self.get_url('/api/jobs')
        payload = {
            "label": "Mobile Submit",
            "source_path": self.src_input.text.strip(),
            "dest_path": self.dst_input.text.strip(),
            "zero_knowledge": self.zk_btn.text == 'ON',
            "encrypt_in_transit": True
        }
        UrlRequest(
            url,
            req_body=json.dumps(payload),
            on_success=lambda req, res: self.refresh_data(),
            req_headers=self.get_headers(),
            method='POST'
        )

    def seed_demo(self, instance):
        url = self.get_url('/api/seed-demo')
        UrlRequest(
            url,
            on_success=lambda req, res: self.refresh_data(),
            req_headers=self.get_headers(),
            method='POST'
        )

class UniFlowApp(App):
    def build(self):
        return UniFlowClient()

if __name__ == '__main__':
    UniFlowApp().run()
