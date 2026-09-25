import { test, expect } from '@playwright/test';
test('software CPU power is used even when BMC has a different reading', async ({ page }) => {
 const id='f21b6aca-d6ab-4323-aebb-95dc84c45d8a';
 await page.route('**/api/v1/**',async route=>{
 const path=new URL(route.request().url()).pathname;
 let body:any=[];
 if(path.endsWith('/me')) body={actor:{id,name:'fixture',token_id:null},csrf:'test',expires_at:'2099-01-01'};
 if(path.endsWith('/machines')) body=[{id,name:'CPU-test',host:'10.0.0.1',tags:[],policy:{},bmc:{provider:'ipmi',url:'10.0.0.2',username:'ADMIN'}}];
 if(path.includes('telemetry')) body=[{machine_id:id,kind:'system',observed_at:new Date().toISOString(),data:{cpu_power_w:307.4,gpus:[],logical_cpus:256},error:null},{machine_id:id,kind:'power',observed_at:new Date().toISOString(),data:{power_w:600,state:'On'},error:null}];
 await route.fulfill({json:body});
 });
 await page.goto('/machines');
 await expect(page.locator('.hive-machine-power')).toContainText('307 W');
 await expect(page.locator('.hive-machine-power')).not.toContainText('已开机');
 await expect(page.locator('.hive-machine-power')).not.toContainText('600');
 await expect(page.getByRole('region', {name:'矿场统计'})).toContainText('¥4.13');
 await expect(page.getByRole('region', {name:'矿场统计'})).toContainText('0.56 元/度');
 await expect(page.getByText('其他算法', {exact:true})).toHaveCount(0);
 await expect(page.getByText('运行实例', {exact:true})).toHaveCount(0);
});
