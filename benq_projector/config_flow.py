"""Config flow for Projector Control."""

from homeassistant import config_entries
from homeassistant.helpers import config_entry_flow
from homeassistant.const import (
    CONF_HOST,
    CONF_UNIQUE_ID,
    CONF_NAME,
    CONF_PASSWORD,
    CONF_PORT,
    CONF_USERNAME,
)
from homeassistant.helpers.device_registry import format_mac

from .const import DOMAIN


class BenqProjectorFlowHandler(config_entries.ConfigFlow, domain=DOMAIN):
    async def async_step_zeroconf(self, discovery_info: dict):
        """Prepare configuration for a server discovered via Zeroconf."""

        config = {
            CONF_HOST: discovery_info[CONF_HOST],
            CONF_NAME: discovery_info["name"].split(".", 1)[0],
            CONF_PORT: discovery_info[CONF_PORT],
            CONF_UNIQUE_ID: discovery_info["properties"]["id"]
        }

        await self.async_set_unique_id(config[CONF_UNIQUE_ID])
        self._abort_if_unique_id_configured(
            updates={
                CONF_HOST: config[CONF_HOST],
                CONF_PORT: config[CONF_PORT],
            }
        )

        return self.async_create_entry(
            title=f"Projector: {config[CONF_NAME]}", data=config
        )
