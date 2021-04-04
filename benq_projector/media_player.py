from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.components.media_player import DEVICE_CLASS_TV, MediaPlayerEntity
from homeassistant.components.media_player.const import (
    MEDIA_TYPE_CHANNEL,
    SUPPORT_SELECT_SOURCE,
    SUPPORT_TURN_OFF,
    SUPPORT_TURN_ON,
    SUPPORT_VOLUME_SET,
    SUPPORT_VOLUME_MUTE,
    SUPPORT_VOLUME_STEP,
)
from homeassistant.const import (
    CONF_HOST,
    CONF_ID,
    CONF_METHOD,
    CONF_NAME,
    CONF_PORT,
    CONF_TOKEN,
    STATE_OFF,
    STATE_ON,
)

SUPPORT_BENQ_PROJECTOR = (
    SUPPORT_TURN_ON
    | SUPPORT_TURN_OFF
    | SUPPORT_VOLUME_STEP
    | SUPPORT_VOLUME_SET
    | SUPPORT_VOLUME_MUTE
    | SUPPORT_SELECT_SOURCE
)


async def async_setup_entry(hass, config_entry, async_add_entities):
    """Set up the projector from a config entry."""

    host = config_entry.data[CONF_HOST]
    port = config_entry.data[CONF_PORT]

    session = async_get_clientsession(hass)
    async with session.get(f"http://{host}:{port}/status") as response:
        status = await response.json()

    async_add_entities([BenQProjector(config_entry, status)])


class BenQProjector(MediaPlayerEntity):
    def __init__(self, config_entry, status):
        self._config_entry = config_entry
        self._status = status

    @property
    def state(self):
        if self._status['state']['power'] == 'on':
            return STATE_ON
        else:
            return STATE_OFF

    @property
    def is_on(self):
        return self._status['state']['power'] == 'on'

    @property
    def is_volume_muted(self):
        """Boolean if volume is currently muted."""
        if self.is_on:
            return self._status['state']['muted']
        else:
            return None

    @property
    def volume_level(self):
        if self.is_on:
            return self._status['state']['volume'] / self._status['state']['max_volume']
        else:
            return None

    @property
    def name(self):
        return self._status['model']

    @property
    def unique_id(self):
        return self._config_entry.data[CONF_UNIQUE_ID]

    @property
    def source(self):
        if self.is_on:
            return self._status['state']['source']
        else:
            return None

    @property
    def source_list(self):
        """List of available input sources."""
        return list(["RGB", "HDMI", "HDMI2"])

    @property
    def supported_features(self):
        """Flag media player features that are supported."""
        return SUPPORT_BENQ_PROJECTOR

    @property
    def device_class(self):
        """Set the device class to TV."""
        return DEVICE_CLASS_TV

    @property
    def _url(self):
        host = self._config_entry.data[CONF_HOST]
        port = self._config_entry.data[CONF_PORT]
        return f"http://{host}:{port}"

    async def async_update(self):
        session = async_get_clientsession(self.hass)
        async with session.get(f"{self._url}/status") as response:
            self._status = await response.json()

    async def async_turn_on(self):
        session = async_get_clientsession(self.hass)
        async with session.post(f"{self._url}/power/on", timeout=150) as response:
            response = await response.json()

    async def async_turn_off(self):
        session = async_get_clientsession(self.hass)
        async with session.post(f"{self._url}/power/off", timeout=150) as response:
            response = await response.json()

    async def async_select_source(self, source):
        """Select input source."""
        session = async_get_clientsession(self.hass)
        async with session.post(f"{self._url}/source/{source}", timeout=150) as response:
            response = await response.json()

    async def async_set_volume_level(self, volume):
        level = round(volume * self._status['state']['max_volume'])

        session = async_get_clientsession(self.hass)
        async with session.post(f"{self._url}/volume/{level}", timeout=150) as response:
            response = await response.json()

    async def async_mute_volume(self, mute):
        mute_status = "on" if mute else "off"

        session = async_get_clientsession(self.hass)
        async with session.post(f"{self._url}/mute/{mute_status}", timeout=150) as response:
            response = await response.json()

